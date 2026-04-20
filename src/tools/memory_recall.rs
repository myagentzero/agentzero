use super::traits::{Tool, ToolResult};
use crate::memory::{Memory, MemoryCategory};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;

/// Parse a `since` value into a UTC cutoff datetime.
///
/// Accepts relative durations (`"7d"`, `"24h"`, `"1w"`, `"30m"`) or absolute
/// dates (`"2026-04-10"` or full RFC 3339).
fn parse_since(s: &str) -> anyhow::Result<DateTime<Utc>> {
    let s = s.trim();

    // Relative duration: digits followed by a unit letter
    if let Some(rest) = s.strip_suffix('m') {
        if let Ok(n) = rest.parse::<i64>() {
            return Ok(Utc::now() - TimeDelta::minutes(n));
        }
    }
    if let Some(rest) = s.strip_suffix('h') {
        if let Ok(n) = rest.parse::<i64>() {
            return Ok(Utc::now() - TimeDelta::hours(n));
        }
    }
    if let Some(rest) = s.strip_suffix('d') {
        if let Ok(n) = rest.parse::<i64>() {
            return Ok(Utc::now() - TimeDelta::days(n));
        }
    }
    if let Some(rest) = s.strip_suffix('w') {
        if let Ok(n) = rest.parse::<i64>() {
            return Ok(Utc::now() - TimeDelta::weeks(n));
        }
    }

    // Absolute date: YYYY-MM-DD
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return date
            .and_hms_opt(0, 0, 0)
            .map(|dt| dt.and_utc())
            .ok_or_else(|| anyhow::anyhow!("Invalid date: {s}"));
    }

    // Full RFC 3339
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    anyhow::bail!(
        "Unrecognized 'since' format: {s}. Use relative (e.g. '7d', '24h') or absolute (e.g. '2026-04-10' or RFC3339)."
    )
}

/// Let the agent search its own memory
pub struct MemoryRecallTool {
    memory: Arc<dyn Memory>,
}

impl MemoryRecallTool {
    pub fn new(memory: Arc<dyn Memory>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemoryRecallTool {
    fn name(&self) -> &str {
        "memory_recall"
    }

    fn description(&self) -> &str {
        "Search memory. Use when: retrieving prior decisions, user preferences, historical context. Don't use when: answer is already in current context."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords or phrase to search for in memory. If omitted, lists all memories (optionally filtered by category)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default: 5)"
                },
                "category": {
                    "type": "string",
                    "description": "Filter by category: 'core', 'daily', 'conversation', 'system', or a custom category name. If omitted, searches all categories."
                },
                "since": {
                    "type": "string",
                    "description": "Only return memories created after this time. Accepts relative durations (e.g. '7d', '24h', '1w', '30m') or absolute dates (e.g. '2026-04-10' or RFC3339)."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let query = args.get("query").and_then(|v| v.as_str());

        #[allow(clippy::cast_possible_truncation)]
        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(5, |v| v as usize);

        let since_cutoff = match args.get("since").and_then(|v| v.as_str()) {
            Some(s) => Some(parse_since(s)?),
            None => None,
        };

        let category_filter = match args.get("category").and_then(|v| v.as_str()) {
            Some("core") => Some(MemoryCategory::Core),
            Some("daily") => Some(MemoryCategory::Daily),
            Some("conversation") => Some(MemoryCategory::Conversation),
            Some("system") => Some(MemoryCategory::System),
            Some(other) => Some(MemoryCategory::Custom(other.to_string())),
            None => None,
        };

        let has_post_filter = category_filter.is_some() || since_cutoff.is_some();

        let result = if let Some(q) = query {
            let fetch_limit = if has_post_filter {
                (limit * 5).min(100)
            } else {
                limit
            };
            self.memory.recall(q, fetch_limit, None).await
        } else {
            self.memory.list(category_filter.as_ref(), None).await
        };

        match result {
            Ok(entries) => {
                let entries: Vec<_> = entries
                    .into_iter()
                    .filter(|e| {
                        category_filter
                            .as_ref()
                            .map_or(true, |cat| &e.category == cat)
                    })
                    .filter(|e| {
                        since_cutoff.map_or(true, |cutoff| {
                            DateTime::parse_from_rfc3339(&e.timestamp)
                                .map_or(false, |ts| ts.with_timezone(&Utc) >= cutoff)
                        })
                    })
                    .take(limit)
                    .collect();

                if entries.is_empty() {
                    return Ok(ToolResult {
                        success: true,
                        output: "No memories found matching that query.".into(),
                        error: None,
                    });
                }

                let mut output = format!("Found {} memories:\n", entries.len());
                for entry in &entries {
                    let score = entry
                        .score
                        .map_or_else(String::new, |s| format!(" [{s:.0}%]"));
                    let _ = writeln!(
                        output,
                        "- [{}] {}: {}{score}",
                        entry.category, entry.key, entry.content
                    );
                }
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Memory recall failed: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryCategory, SqliteMemory};
    use chrono::{TimeDelta, Utc};
    use tempfile::TempDir;

    fn seeded_mem() -> (TempDir, Arc<dyn Memory>) {
        let tmp = TempDir::new().unwrap();
        let mem = SqliteMemory::new(tmp.path()).unwrap();
        (tmp, Arc::new(mem))
    }

    #[tokio::test]
    async fn recall_empty() {
        let (_tmp, mem) = seeded_mem();
        let tool = MemoryRecallTool::new(mem);
        let result = tool.execute(json!({"query": "anything"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("No memories found"));
    }

    #[tokio::test]
    async fn recall_finds_match() {
        let (_tmp, mem) = seeded_mem();
        mem.store("lang", "User prefers Rust", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.store("tz", "Timezone is EST", MemoryCategory::Core, None)
            .await
            .unwrap();

        let tool = MemoryRecallTool::new(mem);
        let result = tool.execute(json!({"query": "Rust"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Rust"));
        assert!(result.output.contains("Found 1"));
    }

    #[tokio::test]
    async fn recall_respects_limit() {
        let (_tmp, mem) = seeded_mem();
        for i in 0..10 {
            mem.store(
                &format!("k{i}"),
                &format!("Rust fact {i}"),
                MemoryCategory::Core,
                None,
            )
            .await
            .unwrap();
        }

        let tool = MemoryRecallTool::new(mem);
        let result = tool
            .execute(json!({"query": "Rust", "limit": 3}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 3"));
    }

    #[tokio::test]
    async fn recall_missing_query_lists() {
        let (_tmp, mem) = seeded_mem();
        let tool = MemoryRecallTool::new(mem);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("No memories found"));
    }

    #[test]
    fn name_and_schema() {
        let (_tmp, mem) = seeded_mem();
        let tool = MemoryRecallTool::new(mem);
        assert_eq!(tool.name(), "memory_recall");
        assert!(tool.parameters_schema()["properties"]["query"].is_object());
    }

    #[tokio::test]
    async fn recall_filters_by_category() {
        let (_tmp, mem) = seeded_mem();
        mem.store("lang", "User prefers Rust", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.store("morning", "Had coffee meeting", MemoryCategory::Daily, None)
            .await
            .unwrap();
        mem.store(
            "chat",
            "Discussed Rust project",
            MemoryCategory::Conversation,
            None,
        )
        .await
        .unwrap();

        let tool = MemoryRecallTool::new(mem);
        let result = tool
            .execute(json!({"query": "Rust", "category": "core"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 1"));
        assert!(result.output.contains("[core]"));
        assert!(!result.output.contains("[conversation]"));
    }

    #[tokio::test]
    async fn recall_no_query_lists_all() {
        let (_tmp, mem) = seeded_mem();
        mem.store("lang", "User prefers Rust", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.store("tz", "Timezone is EST", MemoryCategory::Daily, None)
            .await
            .unwrap();

        let tool = MemoryRecallTool::new(mem);
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 2"));
    }

    #[tokio::test]
    async fn recall_no_query_with_category() {
        let (_tmp, mem) = seeded_mem();
        mem.store("lang", "User prefers Rust", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.store("tz", "Timezone is EST", MemoryCategory::Daily, None)
            .await
            .unwrap();
        mem.store("pref", "Likes dark mode", MemoryCategory::Core, None)
            .await
            .unwrap();

        let tool = MemoryRecallTool::new(mem);
        let result = tool.execute(json!({"category": "core"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 2"));
        assert!(result.output.contains("[core]"));
        assert!(!result.output.contains("[daily]"));
    }

    #[tokio::test]
    async fn recall_without_category_returns_all() {
        let (_tmp, mem) = seeded_mem();
        mem.store("lang", "User prefers Rust", MemoryCategory::Core, None)
            .await
            .unwrap();
        mem.store(
            "chat",
            "Discussed Rust project",
            MemoryCategory::Conversation,
            None,
        )
        .await
        .unwrap();

        let tool = MemoryRecallTool::new(mem);
        let result = tool.execute(json!({"query": "Rust"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 2"));
    }

    #[tokio::test]
    async fn recall_since_relative_includes_recent() {
        let (_tmp, mem) = seeded_mem();
        mem.store("lang", "User prefers Rust", MemoryCategory::Core, None)
            .await
            .unwrap();

        let tool = MemoryRecallTool::new(mem);
        // Memory was just stored, so "1h" should include it
        let result = tool
            .execute(json!({"query": "Rust", "since": "1h"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 1"));
    }

    #[tokio::test]
    async fn recall_since_absolute_future_excludes_all() {
        let (_tmp, mem) = seeded_mem();
        mem.store("lang", "User prefers Rust", MemoryCategory::Core, None)
            .await
            .unwrap();

        let tool = MemoryRecallTool::new(mem);
        // A future date should exclude everything
        let result = tool
            .execute(json!({"query": "Rust", "since": "2099-01-01"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("No memories found"));
    }

    #[test]
    fn parse_since_relative_durations() {
        let now = Utc::now();

        let cutoff = parse_since("7d").unwrap();
        assert!(now - cutoff < TimeDelta::days(7) + TimeDelta::seconds(2));
        assert!(now - cutoff > TimeDelta::days(7) - TimeDelta::seconds(2));

        let cutoff = parse_since("24h").unwrap();
        assert!(now - cutoff < TimeDelta::hours(24) + TimeDelta::seconds(2));

        let cutoff = parse_since("30m").unwrap();
        assert!(now - cutoff < TimeDelta::minutes(30) + TimeDelta::seconds(2));

        let cutoff = parse_since("1w").unwrap();
        assert!(now - cutoff < TimeDelta::weeks(1) + TimeDelta::seconds(2));
    }

    #[test]
    fn parse_since_absolute_date() {
        let cutoff = parse_since("2026-04-10").unwrap();
        assert_eq!(cutoff.date_naive().to_string(), "2026-04-10");
    }

    #[test]
    fn parse_since_rfc3339() {
        let cutoff = parse_since("2026-04-10T12:00:00Z").unwrap();
        assert_eq!(cutoff.to_rfc3339().contains("2026-04-10"), true);
    }

    #[test]
    fn parse_since_invalid() {
        assert!(parse_since("notadate").is_err());
        assert!(parse_since("").is_err());
    }
}
