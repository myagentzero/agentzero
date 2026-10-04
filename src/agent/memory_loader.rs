use crate::memory::{self, Memory, decay::DecayHalfLives, retrieval};
use async_trait::async_trait;
use std::fmt::Write;

#[async_trait]
pub trait MemoryLoader: Send + Sync {
    async fn load_context(&self, memory: &dyn Memory, user_message: &str)
    -> anyhow::Result<String>;
}

pub struct DefaultMemoryLoader {
    limit: usize,
    min_relevance_score: f64,
    half_lives: DecayHalfLives,
}

impl Default for DefaultMemoryLoader {
    fn default() -> Self {
        Self {
            limit: 5,
            min_relevance_score: 0.4,
            half_lives: DecayHalfLives::default(),
        }
    }
}

impl DefaultMemoryLoader {
    pub fn new(limit: usize, min_relevance_score: f64, half_lives: DecayHalfLives) -> Self {
        Self {
            limit: limit.max(1),
            min_relevance_score,
            half_lives,
        }
    }
}

#[async_trait]
impl MemoryLoader for DefaultMemoryLoader {
    async fn load_context(
        &self,
        memory: &dyn Memory,
        user_message: &str,
    ) -> anyhow::Result<String> {
        let entries =
            retrieval::ranked_recall(memory, user_message, self.limit, None, &self.half_lives)
                .await?;

        let scored: Vec<_> = entries
            .iter()
            .filter(|e| !memory::is_assistant_autosave_key(&e.key))
            .filter(|e| e.score.map_or(true, |s| s >= self.min_relevance_score))
            .take(self.limit)
            .collect();

        if scored.is_empty() {
            return Ok(String::new());
        }

        let mut context = String::from("[Memory context]\n");
        for entry in &scored {
            let _ = writeln!(context, "- {}: {}", entry.key, entry.content);
        }
        context.push('\n');
        Ok(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};
    use chrono::Utc;
    use std::sync::Arc;

    fn recent_rfc3339() -> String {
        Utc::now().to_rfc3339()
    }

    fn days_ago_rfc3339(days: i64) -> String {
        (Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    struct MockMemory;
    struct MockMemoryWithEntries {
        entries: Arc<Vec<MemoryEntry>>,
    }
    struct FailingRecallMemory;

    #[async_trait]
    impl Memory for MockMemory {
        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            if limit == 0 {
                return Ok(vec![]);
            }
            Ok(vec![MemoryEntry {
                id: "1".into(),
                key: "k".into(),
                content: "v".into(),
                category: MemoryCategory::Conversation,
                timestamp: "now".into(),
                session_id: None,
                score: None,
            }])
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(vec![])
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }

        async fn health_check(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[async_trait]
    impl Memory for MockMemoryWithEntries {
        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(self.entries.as_ref().clone())
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(vec![])
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(self.entries.len())
        }

        async fn health_check(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "mock-with-entries"
        }
    }

    #[async_trait]
    impl Memory for FailingRecallMemory {
        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Err(anyhow::anyhow!("memory backend unavailable"))
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(vec![])
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }

        async fn health_check(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "failing-recall-memory"
        }
    }

    #[tokio::test]
    async fn default_loader_formats_context() {
        let loader = DefaultMemoryLoader::default();
        let context = loader.load_context(&MockMemory, "hello").await.unwrap();
        assert!(context.contains("[Memory context]"));
        assert!(context.contains("- k: v"));
    }

    #[tokio::test]
    async fn default_loader_skips_legacy_assistant_autosave_entries() {
        let loader = DefaultMemoryLoader::new(5, 0.0, DecayHalfLives::default());
        let memory = MockMemoryWithEntries {
            entries: Arc::new(vec![
                MemoryEntry {
                    id: "1".into(),
                    key: "assistant_resp_legacy".into(),
                    content: "fabricated detail".into(),
                    category: MemoryCategory::Daily,
                    timestamp: "now".into(),
                    session_id: None,
                    score: Some(0.95),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "user_fact".into(),
                    content: "User prefers concise answers".into(),
                    category: MemoryCategory::Conversation,
                    timestamp: "now".into(),
                    session_id: None,
                    score: Some(0.9),
                },
            ]),
        };

        let context = loader.load_context(&memory, "answer style").await.unwrap();
        assert!(context.contains("user_fact"));
        assert!(!context.contains("assistant_resp_legacy"));
        assert!(!context.contains("fabricated detail"));
    }

    #[tokio::test]
    async fn core_category_boost_promotes_low_score_core_entry() {
        let loader = DefaultMemoryLoader::new(2, 0.4, DecayHalfLives::default());
        let memory = MockMemoryWithEntries {
            entries: Arc::new(vec![
                MemoryEntry {
                    id: "1".into(),
                    key: "chat_detail".into(),
                    content: "talked about weather".into(),
                    category: MemoryCategory::Conversation,
                    timestamp: recent_rfc3339(),
                    session_id: None,
                    score: Some(0.6),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "project_rule".into(),
                    content: "always use async/await".into(),
                    category: MemoryCategory::Core,
                    timestamp: recent_rfc3339(),
                    session_id: None,
                    // Below threshold without boost (0.25 < 0.4),
                    // but above with +0.3 boost (0.55 >= 0.4).
                    score: Some(0.25),
                },
                MemoryEntry {
                    id: "3".into(),
                    key: "low_conv".into(),
                    content: "irrelevant chatter".into(),
                    category: MemoryCategory::Conversation,
                    timestamp: "now".into(),
                    session_id: None,
                    score: Some(0.2),
                },
            ]),
        };

        let context = loader.load_context(&memory, "code style").await.unwrap();
        // Core entry should survive thanks to boost
        assert!(
            context.contains("project_rule"),
            "Core entry should be promoted by boost: {context}"
        );
        // Low-score Conversation entry should be filtered out
        assert!(
            !context.contains("low_conv"),
            "Low-score non-Core entry should be filtered: {context}"
        );
    }

    #[tokio::test]
    async fn core_boost_reranks_above_conversation() {
        let loader = DefaultMemoryLoader::new(1, 0.0, DecayHalfLives::default());
        let memory = MockMemoryWithEntries {
            entries: Arc::new(vec![
                MemoryEntry {
                    id: "1".into(),
                    key: "conv_high".into(),
                    content: "recent conversation".into(),
                    category: MemoryCategory::Conversation,
                    timestamp: "now".into(),
                    session_id: None,
                    score: Some(0.6),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "core_pref".into(),
                    content: "user prefers Rust".into(),
                    category: MemoryCategory::Core,
                    timestamp: recent_rfc3339(),
                    session_id: None,
                    // 0.5 + 0.3 boost = 0.8 > 0.6
                    score: Some(0.5),
                },
            ]),
        };

        let context = loader.load_context(&memory, "language").await.unwrap();
        // With limit=1 and Core boost, Core entry (0.8) should win over Conversation (0.6)
        assert!(
            context.contains("core_pref"),
            "Boosted Core should rank above Conversation: {context}"
        );
        assert!(
            !context.contains("conv_high"),
            "Conversation should be truncated when limit=1: {context}"
        );
    }

    #[tokio::test]
    async fn core_older_than_seven_days_does_not_receive_boost() {
        let loader = DefaultMemoryLoader::new(2, 0.4, DecayHalfLives::default());
        let memory = MockMemoryWithEntries {
            entries: Arc::new(vec![
                MemoryEntry {
                    id: "1".into(),
                    key: "chat_detail".into(),
                    content: "talked about weather".into(),
                    category: MemoryCategory::Conversation,
                    timestamp: "now".into(),
                    session_id: None,
                    score: Some(0.6),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "old_rule".into(),
                    content: "always use async/await".into(),
                    category: MemoryCategory::Core,
                    timestamp: days_ago_rfc3339(8),
                    session_id: None,
                    score: Some(0.25),
                },
            ]),
        };

        let context = loader.load_context(&memory, "code style").await.unwrap();
        assert!(
            context.contains("chat_detail"),
            "conversation above threshold should remain: {context}"
        );
        assert!(
            !context.contains("old_rule"),
            "core older than 7 days should not be promoted by boost: {context}"
        );
    }

    #[tokio::test]
    async fn default_loader_propagates_primary_recall_errors() {
        let loader = DefaultMemoryLoader::default();
        let err = loader
            .load_context(&FailingRecallMemory, "hello")
            .await
            .expect_err("expected memory loader to propagate primary recall failure");
        assert!(err.to_string().contains("memory backend unavailable"));
    }
}
