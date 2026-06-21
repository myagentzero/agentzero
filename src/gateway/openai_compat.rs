//! OpenAI-compatible `/v1/chat/completions` and `/v1/models` endpoints.
//!
//! These endpoints allow AgentZero to act as a drop-in replacement for the
//! OpenAI API, enabling any OpenAI-compatible client (e.g., `openai` Python
//! library, `curl`, Aura) to send chat requests through the gateway.

use super::AppState;
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Maximum body size for chat completions requests (512KB).
/// Chat histories with many messages can be much larger than the default 64KB gateway limit.
pub const CHAT_COMPLETIONS_MAX_BODY_SIZE: usize = 524_288;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiAuthRejection {
    MissingPairingToken,
    NonLocalWithoutAuthLayer,
}

fn evaluate_openai_gateway_auth(
    pairing_required: bool,
    is_loopback_request: bool,
    has_valid_pairing_token: bool,
    has_webhook_secret: bool,
) -> Option<OpenAiAuthRejection> {
    if pairing_required {
        return (!has_valid_pairing_token).then_some(OpenAiAuthRejection::MissingPairingToken);
    }

    if !is_loopback_request && !has_webhook_secret && !has_valid_pairing_token {
        return Some(OpenAiAuthRejection::NonLocalWithoutAuthLayer);
    }

    None
}

// ══════════════════════════════════════════════════════════════════════════════
// REQUEST / RESPONSE TYPES
// ══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
pub struct ChatCompletionsRequest {
    /// Model ID (e.g. "anthropic/claude-sonnet-4"). Falls back to gateway default.
    #[serde(default)]
    pub model: Option<String>,
    /// Conversation messages.
    pub messages: Vec<ChatCompletionsMessage>,
    /// Sampling temperature. Falls back to gateway default.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Whether to stream the response as SSE events.
    #[serde(default)]
    pub stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionsMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionsResponse {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionsChoice>,
    pub usage: ChatCompletionsUsage,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionsChoice {
    pub index: u32,
    pub message: ChatCompletionsResponseMessage,
    pub finish_reason: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionsResponseMessage {
    pub role: &'static str,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionsUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// SSE streaming chunk format.
#[derive(Debug, Serialize)]
struct ChatCompletionsChunk {
    id: String,
    object: &'static str,
    created: u64,
    model: String,
    choices: Vec<ChunkChoice>,
}

#[derive(Debug, Serialize)]
struct ChunkChoice {
    index: u32,
    delta: ChunkDelta,
    finish_reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub object: &'static str,
    pub data: Vec<ModelObject>,
}

#[derive(Debug, Serialize)]
pub struct ModelObject {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: String,
}

// ══════════════════════════════════════════════════════════════════════════════
// HANDLERS
// ══════════════════════════════════════════════════════════════════════════════

/// GET /v1/models — List available models.
pub async fn handle_v1_models(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim();
    let has_valid_pairing_token = !token.is_empty() && state.pairing.is_authenticated(token);
    let is_loopback_request =
        super::is_loopback_request(Some(peer_addr), &headers, state.trust_forwarded_headers);

    match evaluate_openai_gateway_auth(
        state.pairing.require_pairing(),
        is_loopback_request,
        has_valid_pairing_token,
        state.webhook_secret_hash.is_some(),
    ) {
        Some(OpenAiAuthRejection::MissingPairingToken) => {
            let err = serde_json::json!({
                "error": {
                    "message": "Invalid API key",
                    "type": "invalid_request_error",
                    "code": "invalid_api_key"
                }
            });
            return (StatusCode::UNAUTHORIZED, Json(err));
        }
        Some(OpenAiAuthRejection::NonLocalWithoutAuthLayer) => {
            let err = serde_json::json!({
                "error": {
                    "message": "Unauthorized — configure pairing or X-Webhook-Secret for non-local access",
                    "type": "invalid_request_error",
                    "code": "unauthorized"
                }
            });
            return (StatusCode::UNAUTHORIZED, Json(err));
        }
        None => {}
    }

    let response = ModelsResponse {
        object: "list",
        data: vec![ModelObject {
            id: state.model.clone(),
            object: "model",
            created: unix_timestamp(),
            owned_by: "openai".to_string(),
        }],
    };

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap()),
    )
}

// ══════════════════════════════════════════════════════════════════════════════
// HELPERS
// ══════════════════════════════════════════════════════════════════════════════

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ══════════════════════════════════════════════════════════════════════════════
// TESTS
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_completions_request_deserializes_minimal() {
        let json = r#"{"messages": [{"role": "user", "content": "Hello"}]}"#;
        let req: ChatCompletionsRequest = serde_json::from_str(json).unwrap();
        assert!(req.model.is_none());
        assert!(req.temperature.is_none());
        assert!(req.stream.is_none());
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, "user");
        assert_eq!(req.messages[0].content, "Hello");
    }

    #[test]
    fn chat_completions_request_deserializes_full() {
        let json = r#"{
            "model": "anthropic/claude-sonnet-4",
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hi"}
            ],
            "temperature": 0.5,
            "stream": true
        }"#;
        let req: ChatCompletionsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model.as_deref(), Some("anthropic/claude-sonnet-4"));
        assert_eq!(req.temperature, Some(0.5));
        assert_eq!(req.stream, Some(true));
        assert_eq!(req.messages.len(), 2);
    }

    #[test]
    fn chat_completions_response_serializes() {
        let response = ChatCompletionsResponse {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion",
            created: 1_234_567_890,
            model: "test-model".to_string(),
            choices: vec![ChatCompletionsChoice {
                index: 0,
                message: ChatCompletionsResponseMessage {
                    role: "assistant",
                    content: "Hello!".to_string(),
                },
                finish_reason: "stop",
            }],
            usage: ChatCompletionsUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("chatcmpl-test"));
        assert!(json.contains("chat.completion"));
        assert!(json.contains("Hello!"));
        assert!(json.contains("stop"));
    }

    #[test]
    fn models_response_serializes() {
        let response = ModelsResponse {
            object: "list",
            data: vec![ModelObject {
                id: "anthropic/claude-sonnet-4".to_string(),
                object: "model",
                created: 1_234_567_890,
                owned_by: "openai".to_string(),
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"object\":\"list\""));
        assert!(json.contains("anthropic/claude-sonnet-4"));
        assert!(json.contains("openai"));
    }

    #[test]
    fn streaming_chunk_serializes() {
        let chunk = ChatCompletionsChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk",
            created: 1_234_567_890,
            model: "test-model".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: Some("assistant"),
                    content: Some("Hello".to_string()),
                },
                finish_reason: None,
            }],
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("chat.completion.chunk"));
        assert!(json.contains("Hello"));
        assert!(json.contains("assistant"));
    }

    #[test]
    fn streaming_chunk_omits_none_fields() {
        let chunk = ChatCompletionsChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk",
            created: 1_234_567_890,
            model: "test-model".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                },
                finish_reason: None,
            }],
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(!json.contains("role"));
        assert!(!json.contains("content"));
    }

    #[test]
    fn body_size_limit_is_512kb() {
        assert_eq!(CHAT_COMPLETIONS_MAX_BODY_SIZE, 524_288);
    }
}
