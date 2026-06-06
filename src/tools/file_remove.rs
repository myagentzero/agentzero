use super::traits::{Tool, ToolResult};
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

const BOOTSTRAP_FILES: &[&str] = &[
    "AGENTS.md",
    "HEARTBEAT.md",
    "IDENTITY.md",
    "SOUL.md",
    "TOOLS.md",
    "USER.md",
    "MEMORY.md",
    "SECURITY.md",
];

const PROTECTED_DIRS: &[&str] = &["memory", "state", "sessions", "cron"];

pub struct FileRemoveTool {
    security: Arc<SecurityPolicy>,
}

impl FileRemoveTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self {
        Self { security }
    }

    fn is_protected_path(&self, resolved: &Path) -> bool {
        let workspace = &self.security.workspace_dir;
        let Ok(workspace_canonical) = std::fs::canonicalize(workspace) else {
            return false;
        };

        let Ok(stripped) = resolved.strip_prefix(&workspace_canonical) else {
            return false;
        };

        if let Some(file_name) = stripped.file_name().and_then(|n| n.to_str()) {
            if (stripped
                .parent()
                .map_or(false, |p| p.as_os_str().is_empty())
                || stripped.parent().is_none())
                && BOOTSTRAP_FILES
                    .iter()
                    .any(|b| b.eq_ignore_ascii_case(file_name))
            {
                return true;
            }
        }

        for component in stripped.components() {
            if let std::path::Component::Normal(seg) = component {
                if let Some(s) = seg.to_str() {
                    if PROTECTED_DIRS.iter().any(|d| d.eq_ignore_ascii_case(s)) {
                        return true;
                    }
                }
            }
        }

        false
    }
}

#[async_trait]
impl Tool for FileRemoveTool {
    fn name(&self) -> &str {
        "file_remove"
    }

    fn description(&self) -> &str {
        "Permanently remove a workspace file (no directories, bootstrap configs)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to remove. Relative paths resolve from workspace."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

        if !self.security.can_act() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: autonomy is read-only".into()),
            });
        }

        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        if !self.security.is_path_allowed(path) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path not allowed by security policy: {path}")),
            });
        }

        let full_path = self.security.resolve_tool_path(path);

        let resolved = match tokio::fs::canonicalize(&full_path).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("File not found or cannot resolve path: {e}")),
                });
            }
        };

        if !self.security.is_resolved_path_allowed(&resolved) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(self.security.resolved_path_violation_message(&resolved)),
            });
        }

        let meta = match tokio::fs::symlink_metadata(&resolved).await {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Cannot read file metadata: {e}")),
                });
            }
        };

        if meta.is_dir() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Cannot remove a directory — only files are allowed".into()),
            });
        }

        if meta.file_type().is_symlink() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Refusing to remove symlink: {}",
                    resolved.display()
                )),
            });
        }

        if self.is_protected_path(&resolved) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Cannot remove protected file: {}", path)),
            });
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        match tokio::fs::remove_file(&resolved).await {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!("Removed {path}"),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to remove file: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{AutonomyLevel, SecurityPolicy};

    fn test_security(workspace: std::path::PathBuf) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: workspace,
            ..SecurityPolicy::default()
        })
    }

    fn test_security_with(
        workspace: std::path::PathBuf,
        autonomy: AutonomyLevel,
        max_actions_per_hour: u32,
    ) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy,
            workspace_dir: workspace,
            max_actions_per_hour,
            ..SecurityPolicy::default()
        })
    }

    #[test]
    fn file_remove_name() {
        let tool = FileRemoveTool::new(test_security(std::env::temp_dir()));
        assert_eq!(tool.name(), "file_remove");
    }

    #[test]
    fn file_remove_schema_has_path() {
        let tool = FileRemoveTool::new(test_security(std::env::temp_dir()));
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["path"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("path")));
    }

    #[tokio::test]
    async fn file_remove_deletes_file() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("trash.txt"), "bye")
            .await
            .unwrap();

        let tool = FileRemoveTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "trash.txt"})).await.unwrap();
        assert!(result.success, "should succeed: {:?}", result.error);
        assert!(!dir.join("trash.txt").exists());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_remove_blocks_nonexistent() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove_noexist");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let tool = FileRemoveTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "ghost.txt"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("not found"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_remove_blocks_directory() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove_dir");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(dir.join("subdir")).await.unwrap();

        let tool = FileRemoveTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "subdir"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("directory"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_remove_blocks_readonly_mode() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove_readonly");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("keep.txt"), "safe")
            .await
            .unwrap();

        let tool =
            FileRemoveTool::new(test_security_with(dir.clone(), AutonomyLevel::ReadOnly, 20));
        let result = tool.execute(json!({"path": "keep.txt"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("read-only"));
        assert!(dir.join("keep.txt").exists());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_remove_blocks_rate_limited() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove_rate");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("keep.txt"), "safe")
            .await
            .unwrap();

        let tool = FileRemoveTool::new(test_security_with(
            dir.clone(),
            AutonomyLevel::Supervised,
            0,
        ));
        let result = tool.execute(json!({"path": "keep.txt"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("Rate limit"));
        assert!(dir.join("keep.txt").exists());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_remove_blocks_path_traversal() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove_traversal");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let tool = FileRemoveTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "../../etc/passwd"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("not allowed")
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_remove_blocks_bootstrap_file() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove_bootstrap");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        for name in BOOTSTRAP_FILES {
            tokio::fs::write(dir.join(name), "core").await.unwrap();
        }

        let tool = FileRemoveTool::new(test_security(dir.clone()));

        for name in BOOTSTRAP_FILES {
            let result = tool.execute(json!({"path": name})).await.unwrap();
            assert!(
                !result.success,
                "should block removal of {name}: {:?}",
                result.error
            );
            assert!(
                result.error.as_deref().unwrap_or("").contains("protected"),
                "error for {name} should mention protected"
            );
            assert!(dir.join(name).exists(), "{name} must still exist");
        }

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_remove_blocks_protected_dir() {
        let dir = std::env::temp_dir().join("zeroclaw_test_file_remove_protdir");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        for protected in PROTECTED_DIRS {
            let sub = dir.join(protected);
            tokio::fs::create_dir_all(&sub).await.unwrap();
            tokio::fs::write(sub.join("data.db"), "important")
                .await
                .unwrap();
        }

        let tool = FileRemoveTool::new(test_security(dir.clone()));

        for protected in PROTECTED_DIRS {
            let path = format!("{protected}/data.db");
            let result = tool.execute(json!({"path": path})).await.unwrap();
            assert!(
                !result.success,
                "should block removal in {protected}/: {:?}",
                result.error
            );
            assert!(
                result.error.as_deref().unwrap_or("").contains("protected"),
                "error for {protected}/ should mention protected"
            );
            assert!(
                dir.join(protected).join("data.db").exists(),
                "{protected}/data.db must still exist"
            );
        }

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_remove_blocks_symlink() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join("zeroclaw_test_file_remove_symlink");
        let workspace = root.join("workspace");
        let outside = root.join("outside");

        let _ = tokio::fs::remove_dir_all(&root).await;
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();

        tokio::fs::write(outside.join("target.txt"), "original")
            .await
            .unwrap();
        symlink(outside.join("target.txt"), workspace.join("linked.txt")).unwrap();

        let tool = FileRemoveTool::new(test_security(workspace.clone()));
        let result = tool.execute(json!({"path": "linked.txt"})).await.unwrap();

        assert!(!result.success, "removing through symlink must be blocked");
        assert!(
            result.error.as_deref().unwrap_or("").contains("symlink"),
            "error should mention symlink"
        );

        let content = tokio::fs::read_to_string(outside.join("target.txt"))
            .await
            .unwrap();
        assert_eq!(content, "original", "original file must not be removed");

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn file_remove_missing_path_param() {
        let tool = FileRemoveTool::new(test_security(std::env::temp_dir()));
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }
}
