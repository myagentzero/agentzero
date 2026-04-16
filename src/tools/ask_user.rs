//! Interactive user prompting tool for cross-channel confirmations.
//!
//! Exposes `ask_user` as an agent-callable tool that sends a question to a
//! messaging channel and waits for the user's response. Resolves channels at
//! execution time via the global `live_channels_registry()`, so no late-binding
//! handle is needed.

use super::traits::{Tool, ToolResult};
use crate::channels::traits::{Channel, ChannelMessage, SendMessage};
use crate::security::SecurityPolicy;
use crate::security::policy::ToolOperation;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

/// Agent-callable tool for sending a question to a user and waiting for their response.
pub struct AskUserTool {
    security: Arc<SecurityPolicy>,
    config: crate::config::AskUserConfig,
}

impl AskUserTool {
    pub fn new(security: Arc<SecurityPolicy>, config: crate::config::AskUserConfig) -> Self {
        Self { security, config }
    }
}

/// Format a question with optional choices for display.
fn format_question(question: &str, choices: Option<&[String]>) -> String {
    let mut lines = Vec::new();
    lines.push(format!("**{question}**"));

    if let Some(choices) = choices {
        lines.push(String::new());
        for (i, choice) in choices.iter().enumerate() {
            lines.push(format!("{}. {choice}", i + 1));
        }
        lines.push(String::new());
        lines.push("_Reply with a number or type your answer._".to_string());
    }

    lines.join("\n")
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their response. \
         Sends the question to a messaging channel and blocks until the user replies \
         or the timeout expires. Optionally provide choices for structured responses."
    }

    fn prompt_hint(&self) -> Option<&str> {
        Some(
            "Ask the user a question and wait for a reply. Use when: you need clarification, confirmation, or input from the user. Don't use when: the answer is already in context.",
        )
    }

    fn prompt_hint_compact(&self) -> &str {
        "Ask the user a question and wait for a reply."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "choices": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of choices"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": format!("Seconds to wait for a response (default: {})", self.config.default_timeout_secs)
                },
                "channel": {
                    "type": "string",
                    "description": format!("Target channel name. Defaults to the preferred channel ({}) if omitted.", self.config.default_channel.as_deref().unwrap_or("none"))
                },
                "recipient": {
                    "type": "string",
                    "description": "The conversation/channel ID to send the question to and listen for a reply from. Auto-populated from channel context when omitted."
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        // Security gate: Act operation
        if let Err(e) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "ask_user")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Action blocked: {e}")),
            });
        }

        // Parse required params
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'question' parameter"))?
            .to_string();

        let choices: Option<Vec<String>> = args.get("choices").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
        });

        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.config.default_timeout_secs);

        let requested_channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());

        let recipient = args
            .get("recipient")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        // Resolve channel from the global registry — block-scoped to drop the
        // Mutex guard before any `.await`.
        let (channel_name, channel): (String, Arc<dyn Channel>) = {
            let map = crate::channels::live_channels_registry()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if map.is_empty() {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("No channels available yet (channels not initialized)".to_string()),
                });
            }
            if let Some(ref name) = requested_channel {
                let ch = map.get(name.as_str()).cloned().ok_or_else(|| {
                    let available: Vec<String> = map.keys().cloned().collect();
                    anyhow::anyhow!(
                        "Channel '{}' not found. Available: {}",
                        name,
                        available.join(", ")
                    )
                })?;
                (name.clone(), ch)
            } else {
                // Prefer the configured default_channel if available.
                let preferred = self
                    .config
                    .default_channel
                    .as_deref()
                    .and_then(|name: &str| map.get(name).map(|ch| (name.to_string(), ch.clone())));
                if let Some(pair) = preferred {
                    pair
                } else {
                    let (name, ch) = map.iter().next().ok_or_else(|| {
                        anyhow::anyhow!("No channels available. Configure at least one channel.")
                    })?;
                    (name.clone(), ch.clone())
                }
            }
        };

        // Format and send the question
        let text = format_question(&question, choices.as_deref());
        let msg = SendMessage::new(&text, recipient.as_deref().unwrap_or(""));
        if let Err(e) = channel.send(&msg).await {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Failed to send question to channel '{channel_name}': {e}"
                )),
            });
        }

        // Listen for user response with timeout
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ChannelMessage>(16);
        let timeout = std::time::Duration::from_secs(timeout_secs);

        // Spawn a listener task on the channel
        let listen_channel = Arc::clone(&channel);
        let listen_handle = tokio::spawn(async move { listen_channel.listen(tx).await });

        // Wait for a message matching our target conversation
        let response = tokio::time::timeout(timeout, async {
            while let Some(msg) = rx.recv().await {
                // If we have a recipient, only accept messages from that conversation
                if let Some(ref target) = recipient {
                    if msg.reply_target != *target {
                        tracing::debug!(
                            "ask_user: ignoring message from {} (waiting for {})",
                            msg.reply_target,
                            target
                        );
                        continue;
                    }
                }
                return Some(msg);
            }
            None
        })
        .await;

        // Abort the listener once we have a response or timeout
        listen_handle.abort();

        match response {
            Ok(Some(msg)) => Ok(ToolResult {
                success: true,
                output: msg.content,
                error: None,
            }),
            Ok(None) => Ok(ToolResult {
                success: false,
                output: "TIMEOUT".to_string(),
                error: Some("Channel closed before receiving a response".to_string()),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: "TIMEOUT".to_string(),
                error: Some(format!(
                    "No response received within {timeout_secs} seconds"
                )),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::traits::Channel;
    use parking_lot::RwLock;

    /// A stub channel that records sent messages but never produces incoming messages.
    struct SilentChannel {
        channel_name: String,
        sent: Arc<RwLock<Vec<String>>>,
    }

    impl SilentChannel {
        fn new(name: &str) -> Self {
            Self {
                channel_name: name.to_string(),
                sent: Arc::new(RwLock::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl Channel for SilentChannel {
        fn name(&self) -> &str {
            &self.channel_name
        }

        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent.write().push(message.content.clone());
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            // Never sends anything — simulates no user response
            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
            Ok(())
        }
    }

    /// A stub channel that immediately responds with a canned message.
    struct RespondingChannel {
        channel_name: String,
        reply_target: String,
        response: String,
        sent: Arc<RwLock<Vec<String>>>,
    }

    impl RespondingChannel {
        fn new(name: &str, reply_target: &str, response: &str) -> Self {
            Self {
                channel_name: name.to_string(),
                reply_target: reply_target.to_string(),
                response: response.to_string(),
                sent: Arc::new(RwLock::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl Channel for RespondingChannel {
        fn name(&self) -> &str {
            &self.channel_name
        }

        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent.write().push(message.content.clone());
            Ok(())
        }

        async fn listen(
            &self,
            tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            let msg = ChannelMessage {
                id: "resp_1".to_string(),
                sender: "user".to_string(),
                reply_target: self.reply_target.clone(),
                content: self.response.clone(),
                channel: self.channel_name.clone(),
                timestamp: 1000,
                thread_ts: None,
            };
            let _ = tx.send(msg).await;
            Ok(())
        }
    }

    /// A stub channel that sends messages from multiple conversations.
    /// Used to test reply_target filtering.
    struct MultiConversationChannel {
        channel_name: String,
        messages: Vec<(String, String)>, // (reply_target, content)
    }

    impl MultiConversationChannel {
        fn new(name: &str, messages: Vec<(&str, &str)>) -> Self {
            Self {
                channel_name: name.to_string(),
                messages: messages
                    .into_iter()
                    .map(|(rt, c)| (rt.to_string(), c.to_string()))
                    .collect(),
            }
        }
    }

    #[async_trait]
    impl Channel for MultiConversationChannel {
        fn name(&self) -> &str {
            &self.channel_name
        }

        async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
            Ok(())
        }

        async fn listen(
            &self,
            tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            for (i, (reply_target, content)) in self.messages.iter().enumerate() {
                let msg = ChannelMessage {
                    id: format!("msg_{i}"),
                    sender: "user".to_string(),
                    reply_target: reply_target.clone(),
                    content: content.clone(),
                    channel: self.channel_name.clone(),
                    timestamp: 1000 + i as u64,
                    thread_ts: None,
                };
                if tx.send(msg).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
    }

    /// Serialize tests that mutate the global channel registry.
    static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn populate_registry(channels: Vec<(&str, Arc<dyn Channel>)>) {
        let map: std::collections::HashMap<String, Arc<dyn Channel>> = channels
            .into_iter()
            .map(|(name, ch)| (name.to_string(), ch))
            .collect();
        crate::channels::register_live_channels(&map);
    }

    fn clear_registry() {
        let mut guard = crate::channels::live_channels_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.clear();
    }

    // ── Metadata tests ──

    #[test]
    fn tool_name_and_description() {
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        assert_eq!(tool.name(), "ask_user");
        assert!(!tool.description().is_empty());
        assert!(tool.description().contains("question"));
    }

    #[test]
    fn parameter_schema_validation() {
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["question"].is_object());
        assert!(schema["properties"]["choices"].is_object());
        assert!(schema["properties"]["timeout_secs"].is_object());
        assert!(schema["properties"]["channel"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "question"));
        assert!(!required.iter().any(|v| v == "choices"));
        assert!(!required.iter().any(|v| v == "timeout_secs"));
        assert!(!required.iter().any(|v| v == "channel"));
    }

    #[test]
    fn spec_matches_metadata() {
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let spec = tool.spec();
        assert_eq!(spec.name, "ask_user");
        assert_eq!(spec.description, tool.description());
        assert!(spec.parameters["required"].is_array());
    }

    // ── Format question tests ──

    #[test]
    fn format_question_without_choices() {
        let text = format_question("Are you sure?", None);
        assert!(text.contains("Are you sure?"));
        assert!(!text.contains("1."));
    }

    #[test]
    fn format_question_with_choices() {
        let choices = vec!["Yes".to_string(), "No".to_string(), "Maybe".to_string()];
        let text = format_question("Continue?", Some(&choices));
        assert!(text.contains("Continue?"));
        assert!(text.contains("1. Yes"));
        assert!(text.contains("2. No"));
        assert!(text.contains("3. Maybe"));
        assert!(text.contains("Reply with a number"));
    }

    // ── Execute tests ──
    // Tests that touch the global registry are serialized via REGISTRY_LOCK
    // because cargo test runs tests in parallel within a binary.

    #[tokio::test]
    async fn execute_rejects_missing_question() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "test",
            Arc::new(SilentChannel::new("test")) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        clear_registry();
    }

    #[tokio::test]
    async fn execute_rejects_empty_question() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "test",
            Arc::new(SilentChannel::new("test")) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool.execute(json!({ "question": "  " })).await;
        assert!(result.is_err());
        clear_registry();
    }

    #[tokio::test]
    async fn empty_channels_returns_not_initialized() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_registry();
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool.execute(json!({ "question": "Hello?" })).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("not initialized"));
    }

    #[tokio::test]
    async fn unknown_channel_returns_error() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "slack",
            Arc::new(SilentChannel::new("slack")) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool
            .execute(json!({ "question": "Hello?", "channel": "nonexistent" }))
            .await;
        assert!(result.is_err());
        clear_registry();
    }

    #[tokio::test]
    async fn timeout_returns_timeout_output() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "test",
            Arc::new(SilentChannel::new("test")) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool
            .execute(json!({
                "question": "Confirm?",
                "timeout_secs": 1
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert_eq!(result.output, "TIMEOUT");
        assert!(result.error.as_deref().unwrap().contains("1 seconds"));
        clear_registry();
    }

    #[tokio::test]
    async fn successful_response_flow() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "test",
            Arc::new(RespondingChannel::new("test", "C123", "Yes, proceed!")) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool
            .execute(json!({
                "question": "Should we deploy?",
                "recipient": "C123",
                "timeout_secs": 5
            }))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert_eq!(result.output, "Yes, proceed!");
        assert!(result.error.is_none());
        clear_registry();
    }

    #[tokio::test]
    async fn successful_response_with_choices() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "slack",
            Arc::new(RespondingChannel::new("slack", "chat_42", "2")) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool
            .execute(json!({
                "question": "Pick an option",
                "choices": ["Option A", "Option B"],
                "channel": "slack",
                "recipient": "chat_42",
                "timeout_secs": 5
            }))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert_eq!(result.output, "2");
        clear_registry();
    }

    #[tokio::test]
    async fn filters_responses_by_recipient() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Channel sends messages from two conversations: C_OTHER first, then C_TARGET
        populate_registry(vec![(
            "slack",
            Arc::new(MultiConversationChannel::new(
                "slack",
                vec![
                    ("C_OTHER", "wrong conversation"),
                    ("C_OTHER", "also wrong"),
                    ("C_TARGET", "correct response"),
                ],
            )) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        let result = tool
            .execute(json!({
                "question": "Which env?",
                "recipient": "C_TARGET",
                "timeout_secs": 5
            }))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert_eq!(result.output, "correct response");
        clear_registry();
    }

    #[tokio::test]
    async fn no_recipient_accepts_any_message() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "cli",
            Arc::new(RespondingChannel::new("cli", "whatever", "hi")) as Arc<dyn Channel>,
        )]);
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), Default::default());
        // No recipient — accepts the first message regardless of reply_target
        let result = tool
            .execute(json!({
                "question": "Hello?",
                "timeout_secs": 5
            }))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert_eq!(result.output, "hi");
        clear_registry();
    }

    // ── Config-specific tests ──

    #[tokio::test]
    async fn config_default_timeout_is_used() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![(
            "test",
            Arc::new(SilentChannel::new("test")) as Arc<dyn Channel>,
        )]);
        let config = crate::config::AskUserConfig {
            enabled: true,
            default_timeout_secs: 1, // very short
            default_channel: None,
        };
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), config);
        // No explicit timeout_secs arg — should use config's 1s default and time out
        let result = tool.execute(json!({ "question": "Hello?" })).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.output, "TIMEOUT");
        assert!(result.error.as_deref().unwrap().contains("1 seconds"));
        clear_registry();
    }

    #[tokio::test]
    async fn config_default_channel_is_preferred() {
        let _guard = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        populate_registry(vec![
            (
                "slack",
                Arc::new(RespondingChannel::new("slack", "C_SLACK", "from slack"))
                    as Arc<dyn Channel>,
            ),
            (
                "slack",
                Arc::new(RespondingChannel::new("slack", "T_TG", "from slack"))
                    as Arc<dyn Channel>,
            ),
        ]);
        let config = crate::config::AskUserConfig {
            enabled: true,
            default_timeout_secs: 300,
            default_channel: Some("slack".to_string()),
        };
        let tool = AskUserTool::new(Arc::new(SecurityPolicy::default()), config);
        // No explicit channel arg — should prefer "slack" from config
        let result = tool
            .execute(json!({
                "question": "Which channel?",
                "timeout_secs": 5
            }))
            .await
            .unwrap();
        assert!(result.success, "error: {:?}", result.error);
        assert_eq!(result.output, "from slack");
        clear_registry();
    }
}
