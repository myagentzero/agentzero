//! Server-Sent Events (SSE) stream for real-time event delivery.
//!
//! Wraps the broadcast channel in AppState to deliver events to web dashboard clients.

use super::AppState;
use crate::observability::runtime_trace::RuntimeTraceEvent;
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

/// Maximum number of historical events to backfill on SSE connect.
const SSE_BACKFILL_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SseAuthRejection {
    MissingPairingToken,
    NonLocalWithoutAuthLayer,
}

fn evaluate_sse_auth(
    pairing_required: bool,
    is_loopback_request: bool,
    has_valid_pairing_token: bool,
) -> Option<SseAuthRejection> {
    if pairing_required {
        return (!has_valid_pairing_token).then_some(SseAuthRejection::MissingPairingToken);
    }

    if !is_loopback_request && !has_valid_pairing_token {
        return Some(SseAuthRejection::NonLocalWithoutAuthLayer);
    }

    None
}

/// Convert a `RuntimeTraceEvent` to the same JSON shape that `BroadcastObserver` emits.
/// Returns `None` for event types that are not broadcast over SSE.
fn runtime_trace_event_to_sse_json(event: &RuntimeTraceEvent) -> Option<serde_json::Value> {
    let event_type = event.event_type.as_str();
    // Build base event with type, timestamp, and optional fields
    let mut obj = serde_json::json!({
        "type": event_type,
        "timestamp": event.timestamp,
    });

    // Add provider and model if present
    if let Some(provider) = &event.provider {
        obj["provider"] = serde_json::Value::String(provider.clone());
    }
    if let Some(model) = &event.model {
        obj["model"] = serde_json::Value::String(model.clone());
    }
    if let Some(channel) = &event.channel {
        obj["channel"] = serde_json::Value::String(channel.clone());
    }
    if let Some(success) = event.success {
        obj["success"] = serde_json::Value::Bool(success);
    }
    if let Some(message) = &event.message {
        obj["message"] = serde_json::Value::String(message.clone());
    }

    // Add entire payload object
    if !event.payload.is_null() && !event.payload.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        obj["payload"] = event.payload.clone();
    }

    // Only return if this is a known event type
    match event_type {
        "llm_request"
        | "tool_call"
        | "tool_call_start"
        | "error"
        | "agent_start"
        | "agent_end"
        | "llm_response"
        | "turn_complete"
        | "channel_message"
        | "webhook_auth_failure"
        | "heartbeat_tick" => Some(obj),
        _ => None,
    }
}

/// GET /api/events — SSE event stream
pub async fn handle_sse_events(
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

    match evaluate_sse_auth(
        state.pairing.require_pairing(),
        is_loopback_request,
        has_valid_pairing_token,
    ) {
        Some(SseAuthRejection::MissingPairingToken) => {
            return (
                StatusCode::UNAUTHORIZED,
                "Unauthorized — provide Authorization: Bearer <token>",
            )
                .into_response();
        }
        Some(SseAuthRejection::NonLocalWithoutAuthLayer) => {
            return (
                StatusCode::UNAUTHORIZED,
                "Unauthorized — enable gateway pairing or provide a valid paired bearer token for non-local /api/events access",
            )
                .into_response();
        }
        None => {}
    }

    // Subscribe to live events first so we don't miss any during backfill load.
    let rx = state.event_tx.subscribe();

    // Load historical events from runtime trace file for backfill.
    let backfill_events: Vec<Result<Event, Infallible>> = state
        .runtime_trace_path
        .as_ref()
        .and_then(|path| {
            match crate::observability::runtime_trace::load_events(
                path,
                SSE_BACKFILL_LIMIT,
                None,
                None,
            ) {
                Ok(events) => Some(events),
                Err(err) => {
                    tracing::warn!("Failed to load runtime trace for SSE backfill: {err}");
                    None
                }
            }
        })
        .unwrap_or_default()
        .iter()
        .rev() // load_events returns newest-first; reverse to chronological order
        .filter_map(|event| {
            runtime_trace_event_to_sse_json(event)
                .map(|json| Ok(Event::default().data(json.to_string())))
        })
        .collect();

    // Emit a "connected" event first to force an immediate buffer flush
    // and provide end-to-end verification that the stream pipeline works.
    let connected_event = Event::default().data(
        serde_json::json!({
            "type": "connected",
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })
        .to_string(),
    );
    let connected_stream = tokio_stream::iter(vec![Ok::<_, Infallible>(connected_event)]);

    let backfill_stream = tokio_stream::iter(backfill_events);

    let live_stream = BroadcastStream::new(rx).filter_map(
        |result: Result<
            serde_json::Value,
            tokio_stream::wrappers::errors::BroadcastStreamRecvError,
        >| {
            match result {
                Ok(value) => Some(Ok::<_, Infallible>(
                    Event::default().data(value.to_string()),
                )),
                Err(_) => None, // Skip lagged messages
            }
        },
    );

    // Chain: connected → backfill → live
    let stream = connected_stream.chain(backfill_stream).chain(live_stream);

    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response();

    // Anti-buffering headers: prevent proxy/nginx response buffering for SSE.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    // Disables nginx response buffering for streaming.
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );

    response
}

/// Broadcast observer that forwards events to the SSE broadcast channel.
pub struct BroadcastObserver {
    inner: Box<dyn crate::observability::Observer>,
    tx: tokio::sync::broadcast::Sender<serde_json::Value>,
}

impl BroadcastObserver {
    pub fn new(
        inner: Box<dyn crate::observability::Observer>,
        tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    ) -> Self {
        Self { inner, tx }
    }

    /// Borrow the wrapped observer so callers can descend through wrapper layers
    /// (e.g. to locate a concrete backend like `PrometheusObserver` for `/metrics`).
    pub fn inner(&self) -> &dyn crate::observability::Observer {
        self.inner.as_ref()
    }
}

impl crate::observability::Observer for BroadcastObserver {
    fn record_event(&self, event: &crate::observability::ObserverEvent) {
        // Forward to inner observer
        self.inner.record_event(event);

        // Broadcast to SSE subscribers
        let json = match event {
            crate::observability::ObserverEvent::LlmRequest {
                provider,
                model,
                messages_count,
            } => serde_json::json!({
                "type": "llm_request",
                "provider": provider,
                "model": model,
                "messages_count": messages_count,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            crate::observability::ObserverEvent::ToolCall {
                tool,
                duration,
                success,
            } => serde_json::json!({
                "type": "tool_call",
                "tool": tool,
                "duration_ms": duration.as_millis(),
                "success": success,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            crate::observability::ObserverEvent::ToolCallStart { tool } => serde_json::json!({
                "type": "tool_call_start",
                "tool": tool,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            crate::observability::ObserverEvent::Error { component, message } => {
                serde_json::json!({
                    "type": "error",
                    "component": component,
                    "message": message,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })
            }
            crate::observability::ObserverEvent::AgentStart { provider, model } => {
                serde_json::json!({
                    "type": "agent_start",
                    "provider": provider,
                    "model": model,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })
            }
            crate::observability::ObserverEvent::AgentEnd {
                provider,
                model,
                duration,
                tokens_used,
                cost_usd,
            } => serde_json::json!({
                "type": "agent_end",
                "provider": provider,
                "model": model,
                "duration_ms": duration.as_millis(),
                "tokens_used": tokens_used,
                "cost_usd": cost_usd,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            crate::observability::ObserverEvent::LlmResponse {
                provider,
                model,
                input_tokens,
                output_tokens,
                ..
            } => {
                let total = input_tokens.unwrap_or(0) + output_tokens.unwrap_or(0);
                serde_json::json!({
                    "type": "llm_response",
                    "provider": provider,
                    "model": model,
                    "tokens_used": total,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })
            }
            crate::observability::ObserverEvent::TurnComplete => serde_json::json!({
                "type": "turn_complete",
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            crate::observability::ObserverEvent::ChannelMessage { channel, direction } => {
                serde_json::json!({
                    "type": "channel_message",
                    "channel": channel,
                    "direction": direction,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })
            }
            crate::observability::ObserverEvent::WebhookAuthFailure {
                channel,
                signature,
                bearer,
            } => serde_json::json!({
                "type": "webhook_auth_failure",
                "channel": channel,
                "signature": signature,
                "bearer": bearer,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
            crate::observability::ObserverEvent::HeartbeatTick => serde_json::json!({
                "type": "heartbeat_tick",
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }),
        };

        let _ = self.tx.send(json);
    }

    fn record_metric(&self, metric: &crate::observability::traits::ObserverMetric) {
        self.inner.record_metric(metric);
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn name(&self) -> &str {
        "broadcast"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_sse_auth_requires_pairing_token_when_pairing_is_enabled() {
        assert_eq!(
            evaluate_sse_auth(true, true, false),
            Some(SseAuthRejection::MissingPairingToken)
        );
        assert_eq!(evaluate_sse_auth(true, false, true), None);
    }

    #[test]
    fn evaluate_sse_auth_rejects_public_without_auth_layer_when_pairing_disabled() {
        assert_eq!(
            evaluate_sse_auth(false, false, false),
            Some(SseAuthRejection::NonLocalWithoutAuthLayer)
        );
    }

    #[test]
    fn evaluate_sse_auth_allows_loopback_or_valid_token_when_pairing_disabled() {
        assert_eq!(evaluate_sse_auth(false, true, false), None);
        assert_eq!(evaluate_sse_auth(false, false, true), None);
    }

    fn make_trace_event(event_type: &str) -> RuntimeTraceEvent {
        RuntimeTraceEvent {
            id: "test-id".into(),
            timestamp: "2026-03-23T00:00:00Z".into(),
            event_type: event_type.into(),
            channel: None,
            provider: Some("openrouter".into()),
            model: Some("test-model".into()),
            turn_id: None,
            success: Some(true),
            message: Some("test message".into()),
            payload: serde_json::json!({
                "tool": "shell",
                "duration_ms": 42,
                "component": "agent",
                "tokens_used": 100,
                "cost_usd": 0.01
            }),
        }
    }

    #[test]
    fn backfill_conversion_produces_correct_json_for_known_types() {
        let known_types = [
            "llm_request",
            "llm_response",
            "tool_call",
            "tool_call_start",
            "error",
            "agent_start",
            "agent_end",
            "turn_complete",
            "channel_message",
            "webhook_auth_failure",
            "heartbeat_tick",
        ];
        for event_type in &known_types {
            let event = make_trace_event(event_type);
            let json = runtime_trace_event_to_sse_json(&event);
            assert!(json.is_some(), "expected Some for event_type={event_type}");
            let json = json.unwrap();
            assert_eq!(json["type"], *event_type);
            assert_eq!(json["timestamp"], "2026-03-23T00:00:00Z");
        }
    }

    #[test]
    fn backfill_conversion_returns_none_for_unknown_types() {
        let event = make_trace_event("something_unknown");
        assert!(runtime_trace_event_to_sse_json(&event).is_none());
    }

    #[test]
    fn backfill_conversion_extracts_tool_and_duration_from_payload() {
        let event = make_trace_event("tool_call");
        let json = runtime_trace_event_to_sse_json(&event).unwrap();
        assert_eq!(json["payload"]["tool"], "shell");
        assert_eq!(json["payload"]["duration_ms"], 42);
        assert_eq!(json["success"], true);
    }

    #[test]
    fn backfill_conversion_extracts_agent_end_fields() {
        let event = make_trace_event("agent_end");
        let json = runtime_trace_event_to_sse_json(&event).unwrap();
        assert_eq!(json["provider"], "openrouter");
        assert_eq!(json["model"], "test-model");
        assert_eq!(json["payload"]["duration_ms"], 42);
        assert_eq!(json["payload"]["tokens_used"], 100);
    }

    #[test]
    fn backfill_conversion_computes_tokens_from_input_output() {
        let mut event = make_trace_event("llm_response");
        // Simulate the actual runtime trace payload which stores input/output
        // tokens separately, not a pre-computed tokens_used field.
        event.payload = serde_json::json!({
            "input_tokens": 1200,
            "output_tokens": 350,
        });
        let json = runtime_trace_event_to_sse_json(&event).unwrap();
        assert_eq!(json["payload"]["input_tokens"], 1200);
        assert_eq!(json["payload"]["output_tokens"], 350);
    }
}
