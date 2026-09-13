use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Result of a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Broad grouping used to organize tools when presenting them to the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolCategory {
    /// Reading, writing, and searching files and directories.
    FileTools,
    /// Shell execution, version control, and source-hosting integrations.
    DeveloperTools,
    /// Browser automation, HTTP requests, and web search/fetch.
    WebTools,
    /// Scheduling, background jobs, and runtime/provider status.
    RuntimeTools,
    /// Long-term memory storage and recall.
    MemoryTools,
    /// Task tracking, delegation, sub-agents, and multi-step pipelines.
    OrchestrationTools,
    /// Third-party service integrations (Jira, Notion, GitHub API, etc).
    IntegrationTools,
    /// Tools discovered from configured Model Context Protocol servers.
    McpTools,
    /// User interaction and messaging.
    CommunicationTools,
    /// Hardware peripherals and embedded boards.
    HardwareTools,
    /// Small standalone utilities (calculator, weather, local context).
    UtilityTools,
    /// Fallback for tools that don't fit another category.
    #[default]
    Other,
}

impl fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::FileTools => "File Tools",
            Self::DeveloperTools => "Developer Tools",
            Self::WebTools => "Web Tools",
            Self::RuntimeTools => "Runtime Tools",
            Self::MemoryTools => "Memory Tools",
            Self::OrchestrationTools => "Orchestration Tools",
            Self::IntegrationTools => "Integration Tools",
            Self::McpTools => "MCP Tools",
            Self::CommunicationTools => "Communication Tools",
            Self::HardwareTools => "Hardware Tools",
            Self::UtilityTools => "Utility Tools",
            Self::Other => "Other Tools",
        };
        f.write_str(label)
    }
}

/// Description of a tool for the LLM
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub category: ToolCategory,
}

/// Core tool trait — implement for any capability
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used in LLM function calling)
    fn name(&self) -> &str;

    /// Human-readable description
    fn description(&self) -> &str;

    /// JSON schema for parameters
    fn parameters_schema(&self) -> serde_json::Value;

    /// Category used to group this tool in LLM-facing tool instructions.
    fn category(&self) -> ToolCategory {
        ToolCategory::Other
    }

    /// Execute the tool with given arguments
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult>;

    /// Get the full spec for LLM registration
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
            category: self.category(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            "dummy_tool"
        }

        fn description(&self) -> &str {
            "A deterministic test tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                }
            })
        }

        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            Ok(ToolResult {
                success: true,
                output: args
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                error: None,
            })
        }
    }

    #[test]
    fn spec_uses_tool_metadata_and_schema() {
        let tool = DummyTool;
        let spec = tool.spec();

        assert_eq!(spec.name, "dummy_tool");
        assert_eq!(spec.description, "A deterministic test tool");
        assert_eq!(spec.parameters["type"], "object");
        assert_eq!(spec.parameters["properties"]["value"]["type"], "string");
    }

    #[test]
    fn mcp_category_has_distinct_display_name() {
        assert_eq!(ToolCategory::McpTools.to_string(), "MCP Tools");
    }

    #[tokio::test]
    async fn execute_returns_expected_output() {
        let tool = DummyTool;
        let result = tool
            .execute(serde_json::json!({ "value": "hello-tool" }))
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output, "hello-tool");
        assert!(result.error.is_none());
    }

    #[test]
    fn tool_result_serialization_roundtrip() {
        let result = ToolResult {
            success: false,
            output: String::new(),
            error: Some("boom".into()),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();

        assert!(!parsed.success);
        assert_eq!(parsed.error.as_deref(), Some("boom"));
    }
}
