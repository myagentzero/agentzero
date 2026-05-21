use std::time::Duration;

use futures_util::{FutureExt, future::join_all};
use serde_json::Value;
use std::panic::AssertUnwindSafe;
use tracing::info;

use crate::channels::traits::ChannelMessage;
use crate::config::HooksConfig;
use crate::plugins::traits::PluginCapability;
use crate::providers::traits::ChatMessage;
use crate::tools::traits::ToolResult;

use super::traits::{HookHandler, HookResult};

/// Dispatcher that manages registered hook handlers.
///
/// Void hooks are dispatched in parallel via `join_all`.
/// Modifying hooks run sequentially by priority (higher first), piping output
/// and short-circuiting on `Cancel`.
pub struct HookRunner {
    handlers: Vec<Box<dyn HookHandler>>,
}

impl HookRunner {
    /// Create an empty runner with no handlers.
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Build a hook runner from configuration, registering enabled built-in hooks.
    ///
    /// Returns `None` if hooks are disabled in config.
    pub fn from_config(config: &HooksConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let mut runner = Self::new();
        if config.builtin.boot_script {
            runner.register(Box::new(super::builtin::BootScriptHook));
        }
        if config.builtin.command_logger {
            runner.register(Box::new(super::builtin::CommandLoggerHook::new()));
        }
        if config.builtin.session_memory {
            runner.register(Box::new(super::builtin::SessionMemoryHook));
        }
        Some(runner)
    }

    /// Register a handler and re-sort by descending priority.
    pub fn register(&mut self, handler: Box<dyn HookHandler>) {
        self.handlers.push(handler);
        self.handlers
            .sort_by_key(|h| std::cmp::Reverse(h.priority()));
    }

    // ---------------------------------------------------------------
    // Void dispatchers (parallel, fire-and-forget)
    // ---------------------------------------------------------------

    pub async fn fire_gateway_start(&self, host: &str, port: u16) {
        let futs: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_gateway_start(host, port))
            .collect();
        join_all(futs).await;
    }

    pub async fn fire_gateway_stop(&self) {
        let futs: Vec<_> = self.handlers.iter().map(|h| h.on_gateway_stop()).collect();
        join_all(futs).await;
    }

    pub async fn fire_llm_input(&self, messages: &[ChatMessage], model: &str) {
        let futs: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_llm_input(messages, model))
            .collect();
        join_all(futs).await;
    }

    pub async fn fire_after_tool_call(&self, tool: &str, result: &ToolResult, duration: Duration) {
        let futs: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_after_tool_call(tool, result, duration))
            .collect();
        join_all(futs).await;
    }

    // ---------------------------------------------------------------
    // Modifying dispatchers (sequential by priority, short-circuit on Cancel)
    // ---------------------------------------------------------------

    pub async fn run_before_tool_call(
        &self,
        mut name: String,
        mut args: Value,
    ) -> HookResult<(String, Value)> {
        for h in &self.handlers {
            let hook_name = h.name();
            match AssertUnwindSafe(h.before_tool_call(name.clone(), args.clone()))
                .catch_unwind()
                .await
            {
                Ok(HookResult::Continue((n, a))) => {
                    name = n;
                    args = a;
                }
                Ok(HookResult::Cancel(reason)) => {
                    info!(
                        hook = hook_name,
                        reason, "before_tool_call cancelled by hook"
                    );
                    return HookResult::Cancel(reason);
                }
                Err(_) => {
                    tracing::error!(
                        hook = hook_name,
                        "before_tool_call hook panicked; continuing with previous values"
                    );
                }
            }
        }
        HookResult::Continue((name, args))
    }

    pub async fn run_before_compaction(
        &self,
        mut messages: Vec<ChatMessage>,
    ) -> HookResult<Vec<ChatMessage>> {
        for h in &self.handlers {
            let hook_name = h.name();
            match AssertUnwindSafe(h.before_compaction(messages.clone()))
                .catch_unwind()
                .await
            {
                Ok(HookResult::Continue(next)) => messages = next,
                Ok(HookResult::Cancel(reason)) => {
                    info!(
                        hook = hook_name,
                        reason, "before_compaction cancelled by hook"
                    );
                    return HookResult::Cancel(reason);
                }
                Err(_) => {
                    tracing::error!(
                        hook = hook_name,
                        "before_compaction hook panicked; continuing with previous value"
                    );
                }
            }
        }
        HookResult::Continue(messages)
    }

    pub async fn run_after_compaction(&self, mut summary: String) -> HookResult<String> {
        for h in &self.handlers {
            let hook_name = h.name();
            match AssertUnwindSafe(h.after_compaction(summary.clone()))
                .catch_unwind()
                .await
            {
                Ok(HookResult::Continue(next)) => summary = next,
                Ok(HookResult::Cancel(reason)) => {
                    info!(
                        hook = hook_name,
                        reason, "after_compaction cancelled by hook"
                    );
                    return HookResult::Cancel(reason);
                }
                Err(_) => {
                    tracing::error!(
                        hook = hook_name,
                        "after_compaction hook panicked; continuing with previous value"
                    );
                }
            }
        }
        HookResult::Continue(summary)
    }

    pub async fn run_tool_result_persist(
        &self,
        tool: String,
        mut result: ToolResult,
    ) -> HookResult<ToolResult> {
        for h in &self.handlers {
            let hook_name = h.name();
            let has_modify_cap = h
                .capabilities()
                .contains(&PluginCapability::ModifyToolResults);
            match AssertUnwindSafe(h.tool_result_persist(tool.clone(), result.clone()))
                .catch_unwind()
                .await
            {
                Ok(HookResult::Continue(next_result)) => {
                    if next_result.success != result.success
                        || next_result.output != result.output
                        || next_result.error != result.error
                    {
                        if has_modify_cap {
                            result = next_result;
                        } else {
                            tracing::warn!(
                                hook = hook_name,
                                "hook attempted to modify tool result without ModifyToolResults capability; ignoring modification"
                            );
                        }
                    } else {
                        // No actual modification — pass-through is always allowed.
                        result = next_result;
                    }
                }
                Ok(HookResult::Cancel(reason)) => {
                    if has_modify_cap {
                        info!(
                            hook = hook_name,
                            reason, "tool_result_persist cancelled by hook"
                        );
                        return HookResult::Cancel(reason);
                    } else {
                        tracing::warn!(
                            hook = hook_name,
                            reason,
                            "hook attempted to cancel tool result without ModifyToolResults capability; ignoring cancellation"
                        );
                    }
                }
                Err(_) => {
                    tracing::error!(
                        hook = hook_name,
                        "tool_result_persist hook panicked; continuing with previous value"
                    );
                }
            }
        }
        HookResult::Continue(result)
    }

    pub async fn run_on_message_received(
        &self,
        mut message: ChannelMessage,
    ) -> HookResult<ChannelMessage> {
        for h in &self.handlers {
            let hook_name = h.name();
            match AssertUnwindSafe(h.on_message_received(message.clone()))
                .catch_unwind()
                .await
            {
                Ok(HookResult::Continue(m)) => message = m,
                Ok(HookResult::Cancel(reason)) => {
                    info!(
                        hook = hook_name,
                        reason, "on_message_received cancelled by hook"
                    );
                    return HookResult::Cancel(reason);
                }
                Err(_) => {
                    tracing::error!(
                        hook = hook_name,
                        "on_message_received hook panicked; continuing with previous message"
                    );
                }
            }
        }
        HookResult::Continue(message)
    }

    pub async fn run_on_message_sending(
        &self,
        mut channel: String,
        mut recipient: String,
        mut content: String,
    ) -> HookResult<(String, String, String)> {
        for h in &self.handlers {
            let hook_name = h.name();
            match AssertUnwindSafe(h.on_message_sending(
                channel.clone(),
                recipient.clone(),
                content.clone(),
            ))
            .catch_unwind()
            .await
            {
                Ok(HookResult::Continue((c, r, ct))) => {
                    channel = c;
                    recipient = r;
                    content = ct;
                }
                Ok(HookResult::Cancel(reason)) => {
                    info!(
                        hook = hook_name,
                        reason, "on_message_sending cancelled by hook"
                    );
                    return HookResult::Cancel(reason);
                }
                Err(_) => {
                    tracing::error!(
                        hook = hook_name,
                        "on_message_sending hook panicked; continuing with previous message"
                    );
                }
            }
        }
        HookResult::Continue((channel, recipient, content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct SimplePriorityHook {
        name: String,
        priority: i32,
    }

    #[async_trait]
    impl HookHandler for SimplePriorityHook {
        fn name(&self) -> &str {
            &self.name
        }
        fn priority(&self) -> i32 {
            self.priority
        }
    }

    #[test]
    fn register_and_sort_by_priority() {
        let mut runner = HookRunner::new();
        runner.register(Box::new(SimplePriorityHook {
            name: "low".into(),
            priority: 1,
        }));
        runner.register(Box::new(SimplePriorityHook {
            name: "high".into(),
            priority: 10,
        }));
        runner.register(Box::new(SimplePriorityHook {
            name: "mid".into(),
            priority: 5,
        }));

        let names: Vec<&str> = runner.handlers.iter().map(|h| h.name()).collect();
        assert_eq!(names, vec!["high", "mid", "low"]);
    }

    struct ToolResultMutator {
        name: String,
    }

    #[async_trait]
    impl HookHandler for ToolResultMutator {
        fn name(&self) -> &str {
            &self.name
        }
        fn capabilities(&self) -> &[PluginCapability] {
            &[PluginCapability::ModifyToolResults]
        }
        async fn tool_result_persist(
            &self,
            _tool: String,
            mut result: ToolResult,
        ) -> HookResult<ToolResult> {
            result.output = format!("modified_by_{}", self.name);
            HookResult::Continue(result)
        }
    }

    fn sample_tool_result() -> ToolResult {
        ToolResult {
            success: true,
            output: "original".into(),
            error: None,
        }
    }

    #[tokio::test]
    async fn tool_result_persist_modifies_result() {
        let mut runner = HookRunner::new();
        runner.register(Box::new(ToolResultMutator {
            name: "test_hook".into(),
        }));

        let result = runner
            .run_tool_result_persist("shell".into(), sample_tool_result())
            .await;
        match result {
            HookResult::Continue(r) => {
                assert_eq!(r.output, "modified_by_test_hook");
            }
            HookResult::Cancel(_) => panic!("should not cancel"),
        }
    }
}
