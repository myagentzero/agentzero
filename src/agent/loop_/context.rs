use crate::config::Config;
use crate::memory::{self, Memory, decay::DecayHalfLives, retrieval};
use std::fmt::Write;

/// Max memory entries in context.
const CONTEXT_ENTRY_LIMIT: usize = 5;

/// Build memory context for the current message.
///
/// Uses [`retrieval::ranked_recall`] and filters entries below
/// `min_relevance_score`.
pub(super) async fn build_context(
    mem: &dyn Memory,
    user_msg: &str,
    min_relevance_score: f64,
    session_id: Option<&str>,
    half_lives: &DecayHalfLives,
) -> String {
    let mut context = String::new();

    if let Ok(entries) =
        retrieval::ranked_recall(mem, user_msg, CONTEXT_ENTRY_LIMIT, session_id, half_lives).await
    {
        let scored: Vec<_> = entries
            .iter()
            .filter(|e| !memory::is_assistant_autosave_key(&e.key))
            .filter(|e| e.score.map_or(true, |s| s >= min_relevance_score))
            .take(CONTEXT_ENTRY_LIMIT)
            .collect();

        if !scored.is_empty() {
            context.push_str("[Memory context]\n");
            for entry in &scored {
                let _ = writeln!(context, "- {}: {}", entry.key, entry.content);
            }
            context.push('\n');
        }
    }

    context
}

/// Build memory + hardware RAG context for a user message.
///
/// `hardware_rag` and `board_names` are loaded once at startup by
/// [`load_peripheral_hardware_state`] and threaded through every call so the
/// datasheet directory is not re-read on each turn.
pub(super) async fn build_injected_context(
    config: &Config,
    mem: &dyn Memory,
    message: &str,
    session_id: Option<&str>,
    hardware_rag: Option<&crate::hardware::datasheet::HardwareRag>,
    board_names: &[String],
) -> String {
    let mem_context = build_context(
        mem,
        message,
        config.memory.min_relevance_score,
        session_id,
        &DecayHalfLives::from_config(&config.memory),
    )
    .await;
    let rag_limit = if config.agent.light_context { 2 } else { 5 };
    let hw_context = hardware_rag
        .map(|r| build_hardware_context(r, message, board_names, rag_limit))
        .unwrap_or_default();
    format!("{mem_context}{hw_context}")
}

/// Load the hardware RAG index and configured board names from disk.
///
/// Call once per long-lived entry point (REPL session, daemon message
/// handler) and reuse the result; the load walks the datasheet directory
/// and chunks every file, so per-turn calls are wasteful.
pub(super) fn load_peripheral_hardware_state(
    config: &Config,
) -> (Option<crate::hardware::datasheet::HardwareRag>, Vec<String>) {
    let board_names: Vec<String> = config
        .peripherals
        .boards
        .iter()
        .map(|b| b.board.clone())
        .collect();

    let hardware_rag = config
        .peripherals
        .datasheet_dir
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(|dir| crate::hardware::datasheet::HardwareRag::load(&config.workspace_dir, dir.trim()))
        .and_then(Result::ok)
        .filter(|r| !r.is_empty());

    if let Some(ref rag) = hardware_rag {
        tracing::info!(chunks = rag.len(), "Hardware RAG loaded");
    }

    (hardware_rag, board_names)
}

/// Build hardware docs context from RAG.
///
/// Adds matching pin aliases (for example, "red_led" -> 13) and retrieved chunks.
pub(super) fn build_hardware_context(
    rag: &crate::hardware::datasheet::HardwareRag,
    user_msg: &str,
    boards: &[String],
    chunk_limit: usize,
) -> String {
    if rag.is_empty() || boards.is_empty() {
        return String::new();
    }

    let mut context = String::new();

    // Add matching pin aliases.
    let pin_ctx = rag.pin_alias_context(user_msg, boards);
    if !pin_ctx.is_empty() {
        context.push_str(&pin_ctx);
    }

    let chunks = rag.retrieve(user_msg, boards, chunk_limit);
    if chunks.is_empty() && pin_ctx.is_empty() {
        return String::new();
    }

    if !chunks.is_empty() {
        context.push_str("[Hardware documentation]\n");
    }
    for chunk in chunks {
        let board_tag = chunk.board.as_deref().unwrap_or("generic");
        let _ = writeln!(
            context,
            "--- {} ({}) ---\n{}\n",
            chunk.source, board_tag, chunk.content
        );
    }
    context.push('\n');
    context
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Arc;

    fn recent_rfc3339() -> String {
        Utc::now().to_rfc3339()
    }

    fn days_ago_rfc3339(days: i64) -> String {
        (Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    struct MockMemory {
        entries: Arc<Vec<MemoryEntry>>,
    }

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
            "mock-memory"
        }
    }

    #[tokio::test]
    async fn build_context_promotes_core_entries_with_score_boost() {
        let memory = MockMemory {
            entries: Arc::new(vec![
                MemoryEntry {
                    id: "1".into(),
                    key: "conv_note".into(),
                    content: "small talk".into(),
                    category: MemoryCategory::Conversation,
                    timestamp: recent_rfc3339(),
                    session_id: None,
                    score: Some(0.6),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "core_rule".into(),
                    content: "always provide tests".into(),
                    category: MemoryCategory::Core,
                    timestamp: recent_rfc3339(),
                    session_id: None,
                    score: Some(0.2),
                },
                MemoryEntry {
                    id: "3".into(),
                    key: "conv_low".into(),
                    content: "irrelevant".into(),
                    category: MemoryCategory::Conversation,
                    timestamp: "now".into(),
                    session_id: None,
                    score: Some(0.1),
                },
            ]),
        };

        let context =
            build_context(&memory, "test query", 0.4, None, &DecayHalfLives::default()).await;
        assert!(
            context.contains("core_rule"),
            "expected core boost to include core_rule"
        );
        assert!(
            !context.contains("conv_low"),
            "low-score non-core should be filtered"
        );
    }

    #[tokio::test]
    async fn build_context_does_not_boost_core_older_than_seven_days() {
        let memory = MockMemory {
            entries: Arc::new(vec![MemoryEntry {
                id: "1".into(),
                key: "old_core".into(),
                content: "stale preference".into(),
                category: MemoryCategory::Core,
                timestamp: days_ago_rfc3339(8),
                session_id: None,
                score: Some(0.2),
            }]),
        };

        let context =
            build_context(&memory, "test query", 0.4, None, &DecayHalfLives::default()).await;
        assert!(
            !context.contains("old_core"),
            "core older than 7 days should not get the recency boost: {context}"
        );
    }

    #[tokio::test]
    async fn build_context_keeps_output_limit_at_five_entries() {
        let entries = (0..8)
            .map(|idx| MemoryEntry {
                id: idx.to_string(),
                key: format!("k{idx}"),
                content: format!("v{idx}"),
                category: MemoryCategory::Conversation,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.9 - (idx as f64 * 0.01)),
            })
            .collect::<Vec<_>>();
        let memory = MockMemory {
            entries: Arc::new(entries),
        };

        let context = build_context(&memory, "limit", 0.0, None, &DecayHalfLives::default()).await;
        let listed = context
            .lines()
            .filter(|line| line.starts_with("- "))
            .count();
        assert_eq!(listed, 5, "context output limit should remain 5 entries");
    }
}
