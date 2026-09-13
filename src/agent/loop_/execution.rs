use super::parsing::ParsedToolCall;
use super::{ToolLoopCancelled, scrub_credentials};
use crate::approval::ApprovalManager;
use crate::observability::{Observer, ObserverEvent};
use crate::tools::Tool;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn find_tool<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> Option<&'a dyn Tool> {
    tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
}
async fn execute_one_tool(
    call_name: &str,
    call_arguments: serde_json::Value,
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
) -> Result<ToolExecutionOutcome> {
    observer.record_event(&ObserverEvent::ToolCallStart {
        tool: call_name.to_string(),
    });
    let start = Instant::now();

    let Some(tool) = find_tool(tools_registry, call_name) else {
        let reason = format!("Unknown tool: {call_name}");
        let duration = start.elapsed();
        observer.record_event(&ObserverEvent::ToolCall {
            tool: call_name.to_string(),
            duration,
            success: false,
            output: Some(reason.clone()),
        });
        return Ok(ToolExecutionOutcome {
            output: reason.clone(),
            success: false,
            error_reason: Some(scrub_credentials(&reason)),
            duration,
        });
    };

    super::record_tool_usage_from_context(call_name);

    if let Some(reason) =
        crate::security::estop_guard().and_then(|guard| guard.check_tool(call_name))
    {
        let duration = start.elapsed();
        observer.record_event(&ObserverEvent::ToolCall {
            tool: call_name.to_string(),
            duration,
            success: false,
            output: Some(reason.clone()),
        });
        return Ok(ToolExecutionOutcome {
            output: reason.clone(),
            success: false,
            error_reason: Some(reason),
            duration,
        });
    }

    let tool_future = tool.execute(call_arguments);
    let tool_result = if let Some(token) = cancellation_token {
        tokio::select! {
            () = token.cancelled() => return Err(ToolLoopCancelled.into()),
            result = tool_future => result,
        }
    } else {
        tool_future.await
    };

    match tool_result {
        Ok(r) => {
            let duration = start.elapsed();
            if r.success {
                let output = scrub_credentials(&r.output);
                observer.record_event(&ObserverEvent::ToolCall {
                    tool: call_name.to_string(),
                    duration,
                    success: true,
                    output: Some(output.clone()),
                });
                Ok(ToolExecutionOutcome {
                    output,
                    success: true,
                    error_reason: None,
                    duration,
                })
            } else {
                let reason = r.error.unwrap_or(r.output);
                tracing::warn!(tool = %call_name, error = %reason, "tool returned failure");
                let output = format!("Error: {reason}");
                let scrubbed = scrub_credentials(&output);
                observer.record_event(&ObserverEvent::ToolCall {
                    tool: call_name.to_string(),
                    duration,
                    success: false,
                    output: Some(scrubbed.clone()),
                });
                Ok(ToolExecutionOutcome {
                    output: scrubbed,
                    success: false,
                    error_reason: Some(scrub_credentials(&reason)),
                    duration,
                })
            }
        }
        Err(e) => {
            let duration = start.elapsed();
            tracing::warn!(tool = %call_name, error = %e, "tool execution error");
            let reason = format!("Error executing {call_name}: {e}");
            observer.record_event(&ObserverEvent::ToolCall {
                tool: call_name.to_string(),
                duration,
                success: false,
                output: Some(reason.clone()),
            });
            Ok(ToolExecutionOutcome {
                output: reason.clone(),
                success: false,
                error_reason: Some(scrub_credentials(&reason)),
                duration,
            })
        }
    }
}

pub(super) struct ToolExecutionOutcome {
    pub(super) output: String,
    pub(super) success: bool,
    pub(super) error_reason: Option<String>,
    pub(super) duration: Duration,
}

const FILE_WRITE_TOOLS: &[&str] = &["file_edit", "file_write", "file_append"];

fn extract_write_path<'a>(call: &'a ParsedToolCall) -> Option<&'a str> {
    if FILE_WRITE_TOOLS.contains(&call.name.as_str()) {
        call.arguments.get("path").and_then(|v| v.as_str())
    } else {
        None
    }
}

fn has_conflicting_file_writes(tool_calls: &[ParsedToolCall]) -> bool {
    let paths: Vec<&str> = tool_calls.iter().filter_map(extract_write_path).collect();
    let unique_count = paths.iter().collect::<std::collections::HashSet<_>>().len();
    unique_count < paths.len()
}

pub(super) fn should_execute_tools_in_parallel(
    tool_calls: &[ParsedToolCall],
    approval: Option<&ApprovalManager>,
) -> bool {
    if tool_calls.len() <= 1 {
        return false;
    }

    if let Some(mgr) = approval {
        if tool_calls
            .iter()
            .any(|call| mgr.needs_approval_for_call(&call.name, &call.arguments))
        {
            // Approval-gated calls must keep sequential handling so the caller can
            // enforce CLI prompt/deny policy consistently.
            return false;
        }
    }

    // Concurrent writes to the same file cause a read-modify-write race that
    // corrupts the file. Force sequential execution when any two write-tool
    // calls target the same path.
    if has_conflicting_file_writes(tool_calls) {
        return false;
    }

    true
}

pub(super) async fn execute_tools_parallel(
    tool_calls: &[ParsedToolCall],
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
) -> Result<Vec<ToolExecutionOutcome>> {
    let futures: Vec<_> = tool_calls
        .iter()
        .map(|call| {
            execute_one_tool(
                &call.name,
                call.arguments.clone(),
                tools_registry,
                observer,
                cancellation_token,
            )
        })
        .collect();

    let results = futures_util::future::join_all(futures).await;
    results.into_iter().collect()
}

pub(super) async fn execute_tools_sequential(
    tool_calls: &[ParsedToolCall],
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
) -> Result<Vec<ToolExecutionOutcome>> {
    let mut outcomes = Vec::with_capacity(tool_calls.len());

    for call in tool_calls {
        outcomes.push(
            execute_one_tool(
                &call.name,
                call.arguments.clone(),
                tools_registry,
                observer,
                cancellation_token,
            )
            .await?,
        );
    }

    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::NoopObserver;
    use crate::security::EstopLevel;
    use crate::security::estop::test_support;
    use async_trait::async_trait;

    struct EchoTool {
        name: String,
    }

    impl EchoTool {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Echoes for estop enforcement tests"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
        ) -> anyhow::Result<crate::tools::ToolResult> {
            Ok(crate::tools::ToolResult {
                success: true,
                output: "ok".to_string(),
                error: None,
            })
        }
    }

    #[tokio::test]
    async fn execute_one_tool_soft_refuses_frozen_tool() {
        let guard = test_support::reset_and_get();
        guard
            .engage(EstopLevel::ToolFreeze(vec!["shell".to_string()]))
            .unwrap();

        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool::new("shell"))];
        let observer = NoopObserver;

        let outcome = execute_one_tool(
            "shell",
            serde_json::json!({}),
            &tools_registry,
            &observer,
            None,
        )
        .await
        .expect("execute_one_tool should return a soft refusal, not an error");

        assert!(!outcome.success);
        assert!(outcome.error_reason.unwrap().contains("frozen"));
    }

    #[tokio::test]
    async fn execute_one_tool_resolves_prefixed_mcp_style_name() {
        let tools_registry: Vec<Box<dyn Tool>> =
            vec![Box::new(EchoTool::new("filesystem__read_file"))];
        let observer = NoopObserver;

        let outcome = execute_one_tool(
            "filesystem__read_file",
            serde_json::json!({}),
            &tools_registry,
            &observer,
            None,
        )
        .await
        .expect("prefixed tool name should resolve");

        assert!(outcome.success);
        assert_eq!(outcome.output, "ok");
    }

    #[tokio::test]
    async fn execute_one_tool_soft_refuses_network_tool_on_network_kill() {
        let guard = test_support::reset_and_get();
        guard.engage(EstopLevel::NetworkKill).unwrap();

        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool::new("web_fetch"))];
        let observer = NoopObserver;

        let outcome = execute_one_tool(
            "web_fetch",
            serde_json::json!({}),
            &tools_registry,
            &observer,
            None,
        )
        .await
        .expect("execute_one_tool should return a soft refusal, not an error");

        assert!(!outcome.success);
        assert!(outcome.error_reason.unwrap().contains("network-kill"));
    }

    #[tokio::test]
    async fn execute_one_tool_allows_non_network_tool_during_network_kill() {
        let guard = test_support::reset_and_get();
        guard.engage(EstopLevel::NetworkKill).unwrap();

        let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(EchoTool::new("file_read"))];
        let observer = NoopObserver;

        let outcome = execute_one_tool(
            "file_read",
            serde_json::json!({}),
            &tools_registry,
            &observer,
            None,
        )
        .await
        .expect("execute_one_tool should not error");

        assert!(outcome.success);
    }
}
