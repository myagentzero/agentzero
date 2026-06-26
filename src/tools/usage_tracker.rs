use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const USAGE_FILE: &str = "tool_usage.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsageStats {
    pub call_count: u64,
    pub last_called: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageStore {
    tools: HashMap<String, ToolUsageStats>,
}

fn state_path(workspace_dir: &Path) -> std::path::PathBuf {
    workspace_dir.join("state").join(USAGE_FILE)
}

fn read_store(workspace_dir: &Path) -> UsageStore {
    let path = state_path(workspace_dir);
    let Ok(contents) = fs::read_to_string(&path) else {
        return UsageStore::default();
    };
    serde_json::from_str(&contents).unwrap_or_default()
}

fn write_store(workspace_dir: &Path, store: &UsageStore) {
    let path = state_path(workspace_dir);
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::warn!("tool usage: failed to create state dir: {e}");
            return;
        }
    }
    match serde_json::to_string_pretty(store) {
        Ok(json) => {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, &json).is_ok() {
                if let Err(e) = fs::rename(&tmp, &path) {
                    tracing::warn!("tool usage: failed to write {}: {e}", path.display());
                    let _ = fs::remove_file(&tmp);
                }
            }
        }
        Err(e) => tracing::warn!("tool usage: failed to serialize store: {e}"),
    }
}

/// Record one invocation of `tool_name`. Updates call_count and last_called.
pub fn record_tool_usage(workspace_dir: &Path, tool_name: &str) {
    let mut store = read_store(workspace_dir);
    let entry = store
        .tools
        .entry(tool_name.to_string())
        .or_insert(ToolUsageStats {
            call_count: 0,
            last_called: Utc::now(),
        });
    entry.call_count += 1;
    entry.last_called = Utc::now();
    write_store(workspace_dir, &store);
}

/// Load all tool usage stats, keyed by tool name.
pub fn load_usage_stats(workspace_dir: &Path) -> HashMap<String, ToolUsageStats> {
    read_store(workspace_dir).tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_and_load_tool_usage() {
        let tmp = TempDir::new().unwrap();
        record_tool_usage(tmp.path(), "file_read");
        record_tool_usage(tmp.path(), "file_read");
        record_tool_usage(tmp.path(), "shell");

        let stats = load_usage_stats(tmp.path());
        assert_eq!(stats["file_read"].call_count, 2);
        assert_eq!(stats["shell"].call_count, 1);
    }
}
