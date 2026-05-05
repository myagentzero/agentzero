use super::traits::{Memory, MemoryEntry};

/// Minimum message length (chars) to trigger keyword expansion query.
const MIN_EXPANSION_LENGTH: usize = 30;

/// Minimum word length to keep in keyword extraction.
const MIN_KEYWORD_LENGTH: usize = 4;

/// Enhanced memory retrieval with multi-query expansion.
///
/// 1. Runs the primary recall with the full query.
/// 2. For long messages, extracts significant keywords and runs a second recall,
///    merging results (deduplicated by key, keeping the higher score).
/// 3. Returns the top `limit` entries sorted by score descending.
pub async fn enhanced_recall(
    mem: &dyn Memory,
    query: &str,
    limit: usize,
    session_id: Option<&str>,
) -> anyhow::Result<Vec<MemoryEntry>> {
    // Decide expansion before any I/O so the two independent recalls can run concurrently.
    let expansion = if query.len() >= MIN_EXPANSION_LENGTH {
        let kw = extract_keywords(query);
        if !kw.is_empty() && kw != query.trim() {
            Some(kw)
        } else {
            None
        }
    } else {
        None
    };

    let mut results = match expansion {
        Some(kw) => {
            let (primary, extra) = tokio::join!(
                mem.recall(query, limit, session_id),
                mem.recall(&kw, limit, session_id),
            );
            let mut results = primary?;
            if let Ok(extra) = extra {
                merge_entries(&mut results, extra);
            }
            results
        }
        None => mem.recall(query, limit, session_id).await?,
    };

    // Sort by score descending, take top `limit`
    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    Ok(results)
}

/// Extract significant keywords (length >= 4) from a message.
fn extract_keywords(msg: &str) -> String {
    msg.split_whitespace()
        .filter_map(|w| {
            let clean = w.trim_matches(|c: char| !c.is_alphanumeric());
            if clean.len() >= MIN_KEYWORD_LENGTH {
                Some(clean)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Merge extra entries into results, deduplicating by key (keep highest score).
fn merge_entries(results: &mut Vec<MemoryEntry>, extra: Vec<MemoryEntry>) {
    for entry in extra {
        if let Some(existing) = results.iter_mut().find(|r| r.key == entry.key) {
            if entry.score.unwrap_or(0.0) > existing.score.unwrap_or(0.0) {
                existing.score = entry.score;
            }
        } else {
            results.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::traits::MemoryCategory;
    use async_trait::async_trait;

    #[test]
    fn extract_keywords_filters_short_words() {
        assert_eq!(
            extract_keywords("I want to use PostgreSQL for the database"),
            "want PostgreSQL database"
        );
    }

    #[test]
    fn extract_keywords_strips_punctuation() {
        // trim_matches strips non-alphanumeric from both ends:
        // "config?" -> "config", "settings." -> "settings", "what's" stays (apostrophe is internal)
        assert_eq!(
            extract_keywords("what's the config? check settings."),
            "what's config check settings"
        );
    }

    #[test]
    fn extract_keywords_empty_for_short_words() {
        assert_eq!(extract_keywords("I am ok"), "");
    }

    #[test]
    fn merge_entries_deduplicates_by_key_keeping_higher_score() {
        let mut results = vec![MemoryEntry {
            id: "1".into(),
            key: "db".into(),
            content: "PostgreSQL".into(),
            category: MemoryCategory::Core,
            timestamp: "now".into(),
            session_id: None,
            score: Some(0.6),
        }];
        let extra = vec![
            MemoryEntry {
                id: "1b".into(),
                key: "db".into(),
                content: "PostgreSQL".into(),
                category: MemoryCategory::Core,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.9), // higher
            },
            MemoryEntry {
                id: "2".into(),
                key: "lang".into(),
                content: "Rust".into(),
                category: MemoryCategory::Core,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.7),
            },
        ];
        merge_entries(&mut results, extra);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].score, Some(0.9)); // upgraded
        assert_eq!(results[1].key, "lang"); // new entry added
    }

    struct MockMemory {
        primary_query: String,
        primary: Vec<MemoryEntry>,
        keyword: Vec<MemoryEntry>,
        fail_primary: bool,
        fail_keyword: bool,
    }

    #[async_trait]
    impl Memory for MockMemory {
        async fn store(
            &self,
            _k: &str,
            _c: &str,
            _cat: MemoryCategory,
            _s: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn recall(
            &self,
            query: &str,
            _limit: usize,
            _s: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            // Dispatch by query string so concurrent recalls stay deterministic.
            if query == self.primary_query {
                if self.fail_primary {
                    Err(anyhow::anyhow!("primary recall failed"))
                } else {
                    Ok(self.primary.clone())
                }
            } else if self.fail_keyword {
                Err(anyhow::anyhow!("keyword recall failed"))
            } else {
                Ok(self.keyword.clone())
            }
        }
        async fn get(&self, _k: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }
        async fn list(
            &self,
            _c: Option<&MemoryCategory>,
            _s: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(vec![])
        }
        async fn forget(&self, _k: &str) -> anyhow::Result<bool> {
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

    #[tokio::test]
    async fn enhanced_recall_merges_primary_and_keyword_results() {
        // Long query triggers expansion
        let query = "what database and programming language should we use for this project";
        let mem = MockMemory {
            primary_query: query.into(),
            primary: vec![MemoryEntry {
                id: "1".into(),
                key: "db".into(),
                content: "PostgreSQL".into(),
                category: MemoryCategory::Core,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.7),
            }],
            keyword: vec![MemoryEntry {
                id: "2".into(),
                key: "lang".into(),
                content: "Rust".into(),
                category: MemoryCategory::Conversation,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.6),
            }],
            fail_primary: false,
            fail_keyword: false,
        };

        let results = enhanced_recall(&mem, query, 5, None).await.unwrap();

        assert_eq!(results.len(), 2);
        // "db" has higher score (0.7), ranked first
        assert_eq!(results[0].key, "db");
        assert_eq!(results[0].score, Some(0.7));
        // "lang" from keyword expansion
        assert_eq!(results[1].key, "lang");
        assert_eq!(results[1].score, Some(0.6));
    }

    #[tokio::test]
    async fn enhanced_recall_skips_expansion_for_short_query() {
        let mem = MockMemory {
            primary_query: "database?".into(),
            primary: vec![MemoryEntry {
                id: "1".into(),
                key: "db".into(),
                content: "PostgreSQL".into(),
                category: MemoryCategory::Core,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.7),
            }],
            keyword: vec![MemoryEntry {
                id: "2".into(),
                key: "lang".into(),
                content: "Rust".into(),
                category: MemoryCategory::Conversation,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.6),
            }],
            fail_primary: false,
            fail_keyword: false,
        };

        // Short query — only the primary recall fires, keyword path is skipped.
        let results = enhanced_recall(&mem, "database?", 5, None).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db");
    }

    #[tokio::test]
    async fn enhanced_recall_respects_limit() {
        let query =
            "a very long query that definitely triggers keyword expansion for testing purposes";
        let mem = MockMemory {
            primary_query: query.into(),
            primary: (0..10)
                .map(|i| MemoryEntry {
                    id: format!("{i}"),
                    key: format!("key_{i}"),
                    content: format!("val_{i}"),
                    category: MemoryCategory::Conversation,
                    timestamp: "now".into(),
                    session_id: None,
                    score: Some(0.5 + i as f64 * 0.01),
                })
                .collect(),
            keyword: vec![],
            fail_primary: false,
            fail_keyword: false,
        };

        let results = enhanced_recall(&mem, query, 3, None).await.unwrap();

        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn enhanced_recall_propagates_primary_recall_errors() {
        let query = "long enough query to trigger expansion";
        let mem = MockMemory {
            primary_query: query.into(),
            primary: vec![],
            keyword: vec![],
            fail_primary: true,
            fail_keyword: false,
        };

        let err = enhanced_recall(&mem, query, 5, None)
            .await
            .expect_err("expected primary recall error to propagate");
        assert!(err.to_string().contains("primary recall failed"));
    }

    #[tokio::test]
    async fn enhanced_recall_tolerates_keyword_recall_errors() {
        let query = "what database and programming language should we use for this project";
        let mem = MockMemory {
            primary_query: query.into(),
            primary: vec![MemoryEntry {
                id: "1".into(),
                key: "db".into(),
                content: "PostgreSQL".into(),
                category: MemoryCategory::Core,
                timestamp: "now".into(),
                session_id: None,
                score: Some(0.7),
            }],
            keyword: vec![],
            fail_primary: false,
            fail_keyword: true,
        };

        let results = enhanced_recall(&mem, query, 5, None).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "db");
    }
}
