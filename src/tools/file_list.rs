use super::traits::{Tool, ToolResult};
use crate::security::SecurityPolicy;
use crate::security::sensitive_paths::is_sensitive_file_path;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

pub struct FileListTool {
    security: Arc<SecurityPolicy>,
}

impl FileListTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self {
        Self { security }
    }
}

fn normalize_list_path(path: Option<&str>) -> &str {
    match path.map(str::trim) {
        None | Some("") | Some("/") => ".",
        Some(p) => p,
    }
}

#[async_trait]
impl Tool for FileListTool {
    fn name(&self) -> &str {
        "file_list"
    }

    fn description(&self) -> &str {
        "List workspace files and directories with optional depth; redacts sensitive files. Prefer over shell ls."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list, relative to workspace. Omit, leave blank, or use '/' for workspace root; absolute paths need policy allowlist."
                },
                "depth": {
                    "type": "integer",
                    "description": "Max recursion depth (0=current level only, 1=current+one level of subdirs, -1=unlimited). Default 0."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let path_str = normalize_list_path(args.get("path").and_then(|v| v.as_str()));

        let depth = args
            .get("depth")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;

        if !self.security.is_path_allowed(path_str) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path not allowed by security policy: {path_str}")),
            });
        }

        let full_path = self.security.resolve_tool_path(path_str);

        if !full_path.exists() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path does not exist: {path_str}")),
            });
        }

        if !full_path.is_dir() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path is not a directory: {path_str}")),
            });
        }

        match list_directory(&full_path, depth, 0) {
            Ok(entries) => {
                let output = serde_json::to_string_pretty(&entries)
                    .unwrap_or_else(|_| "[]".to_string());
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to list directory: {e}")),
            }),
        }
    }
}

fn list_directory(path: &Path, max_depth: i32, current_depth: i32) -> anyhow::Result<Value> {
    let mut entries = Vec::new();

    let mut read_dir: Vec<_> = std::fs::read_dir(path)?
        .collect::<Result<Vec<_>, _>>()?;

    read_dir.sort_by(|a, b| {
        let a_name = a.file_name();
        let b_name = b.file_name();
        a_name.cmp(&b_name)
    });

    for entry in read_dir {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().to_string();

        if is_sensitive_file_path(&path) {
            continue;
        }

        if name == ".git" {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        let mut item = json!({
            "name": name,
            "type": if metadata.is_dir() { "directory" } else { "file" },
            "size": metadata.len(),
        });

        if metadata.is_dir() && (max_depth < 0 || current_depth < max_depth) {
            if let Ok(children) = list_directory(&path, max_depth, current_depth + 1) {
                item["children"] = children;
            }
        }

        entries.push(item);
    }

    Ok(Value::Array(entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::AutonomyLevel;

    fn test_security(workspace: std::path::PathBuf) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: workspace,
            ..SecurityPolicy::default()
        })
    }

    #[test]
    fn file_list_name() {
        let tool = FileListTool::new(test_security(std::env::temp_dir()));
        assert_eq!(tool.name(), "file_list");
    }

    #[test]
    fn file_list_schema() {
        let tool = FileListTool::new(test_security(std::env::temp_dir()));
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["depth"].is_object());
        assert!(schema.get("required").is_none());
    }

    #[tokio::test]
    async fn file_list_defaults_to_workspace_root() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_default_root");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("root.txt"), "content")
            .await
            .unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));

        for args in [json!({}), json!({"path": ""}), json!({"path": "/"})] {
            let result = tool.execute(args).await.unwrap();
            assert!(result.success, "listing should succeed: {:?}", result.error);
            let output: Value = serde_json::from_str(&result.output).unwrap();
            let names: Vec<&str> = output
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|e| e["name"].as_str())
                .collect();
            assert!(names.contains(&"root.txt"));
        }

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_lists_files() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        tokio::fs::write(dir.join("file1.txt"), "content")
            .await
            .unwrap();
        tokio::fs::write(dir.join("file2.md"), "content")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.join("subdir")).await.unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "."}))
            .await
            .unwrap();

        assert!(result.success, "listing should succeed: {:?}", result.error);
        let _output: Value = serde_json::from_str(&result.output).unwrap();
        let array = _output.as_array().unwrap();
        assert_eq!(array.len(), 3);

        let names: Vec<&str> = array
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();
        assert!(names.contains(&"file1.txt"));
        assert!(names.contains(&"file2.md"));
        assert!(names.contains(&"subdir"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_filters_env_files() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_filter");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        tokio::fs::write(dir.join(".env"), "SECRET=123")
            .await
            .unwrap();
        tokio::fs::write(dir.join("file.txt"), "content")
            .await
            .unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "."}))
            .await
            .unwrap();

        assert!(result.success);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        let names: Vec<&str> = output
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();

        assert!(!names.contains(&".env"), ".env file should be filtered");
        assert!(names.contains(&"file.txt"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_filters_git_folder() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_filter_git");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        tokio::fs::create_dir(dir.join(".git")).await.unwrap();
        tokio::fs::write(dir.join(".git/config"), "").await.unwrap();
        tokio::fs::write(dir.join("file.txt"), "content")
            .await
            .unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "."}))
            .await
            .unwrap();

        assert!(result.success);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        let names: Vec<&str> = output
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["name"].as_str())
            .collect();

        assert!(!names.contains(&".git"), ".git folder should be filtered");
        assert!(names.contains(&"file.txt"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_respects_depth_zero() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_depth0");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(dir.join("a/b")).await.unwrap();

        tokio::fs::write(dir.join("file1.txt"), "").await.unwrap();
        tokio::fs::write(dir.join("a/file2.txt"), "").await.unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": ".", "depth": 0}))
            .await
            .unwrap();

        assert!(result.success);
        let output_str = result.output;

        assert!(!output_str.contains("file2.txt"));
        assert!(output_str.contains("file1.txt"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_respects_depth_one() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_depth1");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(dir.join("a/b")).await.unwrap();

        tokio::fs::write(dir.join("file1.txt"), "").await.unwrap();
        tokio::fs::write(dir.join("a/file2.txt"), "").await.unwrap();
        tokio::fs::write(dir.join("a/b/file3.txt"), "").await.unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": ".", "depth": 1}))
            .await
            .unwrap();

        assert!(result.success);
        let output_str = &result.output;

        assert!(output_str.contains("file1.txt"));
        assert!(output_str.contains("file2.txt"));
        assert!(!output_str.contains("file3.txt"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_unlimited_depth() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_unlimited");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(dir.join("a/b/c")).await.unwrap();

        tokio::fs::write(dir.join("file1.txt"), "").await.unwrap();
        tokio::fs::write(dir.join("a/file2.txt"), "").await.unwrap();
        tokio::fs::write(dir.join("a/b/file3.txt"), "").await.unwrap();
        tokio::fs::write(dir.join("a/b/c/file4.txt"), "").await.unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": ".", "depth": -1}))
            .await
            .unwrap();

        assert!(result.success);
        let output_str = &result.output;

        assert!(output_str.contains("file1.txt"));
        assert!(output_str.contains("file2.txt"));
        assert!(output_str.contains("file3.txt"));
        assert!(output_str.contains("file4.txt"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_nonexistent_path() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_nonexistent");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "nonexistent"}))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("does not exist"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_list_file_not_directory() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_list_not_dir");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("file.txt"), "content")
            .await
            .unwrap();

        let tool = FileListTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "file.txt"}))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("not a directory"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
