//! Emoji reaction tool for cross-channel message reactions.
//!
//! Exposes `add_reaction` and `remove_reaction` from the [`Channel`] trait as an
//! agent-callable tool. Resolves channels at execution time via the global
//! `live_channels_registry()`, so no late-binding handle is needed.

use super::traits::{Tool, ToolResult};
use crate::security::SecurityPolicy;
use crate::security::policy::ToolOperation;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

/// Agent-callable tool for adding or removing emoji reactions on messages.
pub struct ReactionTool {
    security: Arc<SecurityPolicy>,
}

impl ReactionTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self {
        Self { security }
    }
}

#[async_trait]
impl Tool for ReactionTool {
    fn name(&self) -> &str {
        "reaction"
    }

    fn description(&self) -> &str {
        "Add or remove emoji reaction on a message (channel, message ID, emoji)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "Name of the channel to react in (e.g. 'slack')"
                },
                "channel_id": {
                    "type": "string",
                    "description": "Platform-specific channel/conversation identifier (e.g. Slack channel ID)"
                },
                "message_id": {
                    "type": "string",
                    "description": "Platform-scoped message identifier to react to"
                },
                "emoji": {
                    "type": "string",
                    "description": "Emoji to react with (Unicode character or platform shortcode)"
                },
                "action": {
                    "type": "string",
                    "enum": ["add", "remove"],
                    "description": "Whether to add or remove the reaction (default: 'add')"
                }
            },
            "required": ["channel", "channel_id", "message_id", "emoji"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        // Security gate
        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "reaction")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let channel_name = args
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'channel' parameter"))?;

        let channel_id = args
            .get("channel_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'channel_id' parameter"))?;

        let message_id = args
            .get("message_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'message_id' parameter"))?;

        let emoji = args
            .get("emoji")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'emoji' parameter"))?;

        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("add");

        if action != "add" && action != "remove" {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Invalid action '{action}': must be 'add' or 'remove'"
                )),
            });
        }

        // Resolve the target channel from the global registry.
        let channel = {
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
            match map.get(channel_name) {
                Some(ch) => Arc::clone(ch),
                None => {
                    let available: Vec<String> = map.keys().cloned().collect();
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!(
                            "Channel '{channel_name}' not found. Available channels: {}",
                            available.join(", ")
                        )),
                    });
                }
            }
        };

        let result = if action == "add" {
            channel.add_reaction(channel_id, message_id, emoji).await
        } else {
            channel.remove_reaction(channel_id, message_id, emoji).await
        };

        let past_tense = if action == "remove" {
            "removed"
        } else {
            "added"
        };

        match result {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!(
                    "Reaction {past_tense}: {emoji} on message {message_id} in {channel_name}"
                ),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to {action} reaction: {e}")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::traits::{Channel, ChannelMessage, SendMessage};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Serialise tests that mutate the global live_channels registry.
    static REGISTRY_LOCK: std::sync::LazyLock<Mutex<()>> =
        std::sync::LazyLock::new(|| Mutex::new(()));

    struct MockChannel {
        reaction_added: AtomicBool,
        reaction_removed: AtomicBool,
        last_channel_id: parking_lot::Mutex<Option<String>>,
        fail_on_add: bool,
    }

    impl MockChannel {
        fn new() -> Self {
            Self {
                reaction_added: AtomicBool::new(false),
                reaction_removed: AtomicBool::new(false),
                last_channel_id: parking_lot::Mutex::new(None),
                fail_on_add: false,
            }
        }

        fn failing() -> Self {
            Self {
                reaction_added: AtomicBool::new(false),
                reaction_removed: AtomicBool::new(false),
                last_channel_id: parking_lot::Mutex::new(None),
                fail_on_add: true,
            }
        }
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            "mock"
        }

        async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn add_reaction(
            &self,
            channel_id: &str,
            _message_id: &str,
            _emoji: &str,
        ) -> anyhow::Result<()> {
            if self.fail_on_add {
                return Err(anyhow::anyhow!("API error: rate limited"));
            }
            *self.last_channel_id.lock() = Some(channel_id.to_string());
            self.reaction_added.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn remove_reaction(
            &self,
            channel_id: &str,
            _message_id: &str,
            _emoji: &str,
        ) -> anyhow::Result<()> {
            *self.last_channel_id.lock() = Some(channel_id.to_string());
            self.reaction_removed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Helper: register mock channels in the global registry for the duration of a test.
    /// Acquires REGISTRY_LOCK so tests that share this global state run serially.
    /// Returns a guard that clears the registry (and releases the lock) on drop.
    fn register_test_channels(channels: Vec<(&str, Arc<dyn Channel>)>) -> impl Drop {
        let serial = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let map: std::collections::HashMap<String, Arc<dyn Channel>> = channels
            .into_iter()
            .map(|(name, ch)| (name.to_string(), ch))
            .collect();
        crate::channels::register_live_channels(&map);

        struct Cleanup(std::sync::MutexGuard<'static, ()>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                crate::channels::register_live_channels(&std::collections::HashMap::new());
            }
        }
        Cleanup(serial)
    }

    fn make_tool() -> ReactionTool {
        ReactionTool::new(Arc::new(SecurityPolicy::default()))
    }

    #[test]
    fn tool_metadata() {
        let tool = make_tool();
        assert_eq!(tool.name(), "reaction");
        assert!(!tool.description().is_empty());
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["channel"].is_object());
        assert!(schema["properties"]["channel_id"].is_object());
        assert!(schema["properties"]["message_id"].is_object());
        assert!(schema["properties"]["emoji"].is_object());
        assert!(schema["properties"]["action"].is_object());
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "channel"));
        assert!(required.iter().any(|v| v == "channel_id"));
        assert!(required.iter().any(|v| v == "message_id"));
        assert!(required.iter().any(|v| v == "emoji"));
        // action is optional (defaults to "add")
        assert!(!required.iter().any(|v| v == "action"));
    }

    #[tokio::test]
    async fn add_reaction_success() {
        let mock: Arc<dyn Channel> = Arc::new(MockChannel::new());
        let _guard = register_test_channels(vec![("discord", Arc::clone(&mock))]);
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "discord",
                "channel_id": "ch_001",
                "message_id": "msg_123",
                "emoji": "\u{2705}"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("added"));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn remove_reaction_success() {
        let mock: Arc<dyn Channel> = Arc::new(MockChannel::new());
        let _guard = register_test_channels(vec![("slack", Arc::clone(&mock))]);
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "slack",
                "channel_id": "C0123SLACK",
                "message_id": "msg_456",
                "emoji": "\u{1F440}",
                "action": "remove"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("removed"));
    }

    #[tokio::test]
    async fn unknown_channel_returns_error() {
        let _guard = register_test_channels(vec![(
            "discord",
            Arc::new(MockChannel::new()) as Arc<dyn Channel>,
        )]);
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "nonexistent",
                "channel_id": "ch_x",
                "message_id": "msg_1",
                "emoji": "\u{2705}"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        let err = result.error.as_deref().unwrap();
        assert!(err.contains("not found"));
        assert!(err.contains("discord"));
    }

    #[tokio::test]
    async fn invalid_action_returns_error() {
        let _guard = register_test_channels(vec![(
            "discord",
            Arc::new(MockChannel::new()) as Arc<dyn Channel>,
        )]);
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "discord",
                "channel_id": "ch_001",
                "message_id": "msg_1",
                "emoji": "\u{2705}",
                "action": "toggle"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("toggle"));
    }

    #[tokio::test]
    async fn channel_error_propagated() {
        let mock: Arc<dyn Channel> = Arc::new(MockChannel::failing());
        let _guard = register_test_channels(vec![("discord", mock)]);
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "discord",
                "channel_id": "ch_001",
                "message_id": "msg_1",
                "emoji": "\u{2705}"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("rate limited"));
    }

    #[tokio::test]
    async fn missing_required_params() {
        let _guard = register_test_channels(vec![(
            "test",
            Arc::new(MockChannel::new()) as Arc<dyn Channel>,
        )]);
        let tool = make_tool();

        // Missing channel
        assert!(
            tool.execute(json!({"channel_id": "c1", "message_id": "1", "emoji": "x"}))
                .await
                .is_err()
        );

        // Missing channel_id
        assert!(
            tool.execute(json!({"channel": "test", "message_id": "1", "emoji": "x"}))
                .await
                .is_err()
        );

        // Missing message_id
        assert!(
            tool.execute(json!({"channel": "a", "channel_id": "c1", "emoji": "x"}))
                .await
                .is_err()
        );

        // Missing emoji
        assert!(
            tool.execute(json!({"channel": "a", "channel_id": "c1", "message_id": "1"}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn empty_channels_returns_not_initialized() {
        let _serial = REGISTRY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Clear registry to simulate pre-init state
        crate::channels::register_live_channels(&std::collections::HashMap::new());
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "discord",
                "channel_id": "ch_001",
                "message_id": "msg_1",
                "emoji": "\u{2705}"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("not initialized"));
    }

    #[tokio::test]
    async fn default_action_is_add() {
        let mock = Arc::new(MockChannel::new());
        let mock_ch: Arc<dyn Channel> = Arc::clone(&mock) as Arc<dyn Channel>;
        let _guard = register_test_channels(vec![("test", mock_ch)]);
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "test",
                "channel_id": "ch_test",
                "message_id": "msg_1",
                "emoji": "\u{2705}"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(mock.reaction_added.load(Ordering::SeqCst));
        assert!(!mock.reaction_removed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn channel_id_passed_to_trait_not_channel_name() {
        let mock = Arc::new(MockChannel::new());
        let mock_ch: Arc<dyn Channel> = Arc::clone(&mock) as Arc<dyn Channel>;
        let _guard = register_test_channels(vec![("discord", mock_ch)]);
        let tool = make_tool();

        let result = tool
            .execute(json!({
                "channel": "discord",
                "channel_id": "123456789",
                "message_id": "msg_1",
                "emoji": "\u{2705}"
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(
            mock.last_channel_id.lock().as_deref(),
            Some("123456789"),
            "add_reaction must receive channel_id, not channel name"
        );
    }

    #[test]
    fn spec_matches_metadata() {
        let tool = make_tool();
        let spec = tool.spec();
        assert_eq!(spec.name, "reaction");
        assert_eq!(spec.description, tool.description());
        assert!(spec.parameters["required"].is_array());
    }
}
