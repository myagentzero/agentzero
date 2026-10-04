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
use serde::Serialize;
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
    fn body_size_limit_is_512kb() {
        assert_eq!(CHAT_COMPLETIONS_MAX_BODY_SIZE, 524_288);
    }
}
