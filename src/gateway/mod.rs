//! Axum-based HTTP gateway with proper HTTP/1.1 compliance, body limits, and timeouts.
//!
//! This module replaces the raw TCP implementation with axum for:
//! - Proper HTTP/1.1 parsing and compliance
//! - Content-Length validation (handled by hyper)
//! - Request body size limits (64KB max)
//! - Request timeouts (30s) to prevent slow-loris attacks
//! - Header sanitization (handled by axum/hyper)

pub mod api;
mod openai_compat;
mod openclaw_compat;
pub mod sse;
pub mod static_files;
pub mod ws;

use crate::config::Config;
use crate::cost::CostTracker;
use crate::memory::{self, Memory, MemoryCategory};
use crate::providers::{self, ChatMessage, Provider};
use crate::runtime;
use crate::security::SecurityPolicy;
use crate::security::pairing::{PairingGuard, constant_time_eq, is_public_bind};
use crate::tools::traits::ToolSpec;
use crate::tools::Tool;
use anyhow::{Context, Result};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
};
use futures_util::StreamExt;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use uuid::Uuid;

/// Maximum request body size (64KB) — prevents memory exhaustion
pub const MAX_BODY_SIZE: usize = 65_536;
/// Request timeout (30s) — prevents slow-loris attacks
pub const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Sliding window used by gateway rate limiting.
pub const RATE_LIMIT_WINDOW_SECS: u64 = 60;
/// Fallback max distinct client keys tracked in gateway rate limiter.
pub const RATE_LIMIT_MAX_KEYS_DEFAULT: usize = 10_000;
/// Fallback max distinct idempotency keys retained in gateway memory.
pub const IDEMPOTENCY_MAX_KEYS_DEFAULT: usize = 10_000;

/// Middleware that injects security headers on every HTTP response.
async fn security_headers_middleware(req: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    // Only set Cache-Control if not already set by handler (e.g., SSE uses no-cache)
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    headers.insert(header::X_XSS_PROTECTION, HeaderValue::from_static("0"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    response
}

fn webhook_memory_key() -> String {
    format!("webhook_msg_{}", Uuid::new_v4())
}

fn gateway_message_session_id(msg: &crate::channels::traits::ChannelMessage) -> String {
    match &msg.thread_ts {
        Some(thread_id) => format!("{}_{}_{}", msg.channel, thread_id, msg.sender),
        None => format!("{}_{}", msg.channel, msg.sender),
    }
}

fn hash_webhook_secret(value: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(value.as_bytes());
    hex::encode(digest)
}

/// How often the rate limiter sweeps stale IP entries from its map.
const RATE_LIMITER_SWEEP_INTERVAL_SECS: u64 = 300; // 5 minutes

#[derive(Debug)]
struct SlidingWindowRateLimiter {
    limit_per_window: u32,
    window: Duration,
    max_keys: usize,
    requests: Mutex<(HashMap<String, Vec<Instant>>, Instant)>,
}

impl SlidingWindowRateLimiter {
    fn new(limit_per_window: u32, window: Duration, max_keys: usize) -> Self {
        Self {
            limit_per_window,
            window,
            max_keys: max_keys.max(1),
            requests: Mutex::new((HashMap::new(), Instant::now())),
        }
    }

    fn prune_stale(requests: &mut HashMap<String, Vec<Instant>>, cutoff: Instant) {
        requests.retain(|_, timestamps| {
            timestamps.retain(|t| *t > cutoff);
            !timestamps.is_empty()
        });
    }

    fn allow(&self, key: &str) -> bool {
        if self.limit_per_window == 0 {
            return true;
        }

        let now = Instant::now();
        let cutoff = now.checked_sub(self.window).unwrap_or_else(Instant::now);

        let mut guard = self.requests.lock();
        let (requests, last_sweep) = &mut *guard;

        // Periodic sweep: remove keys with no recent requests
        if last_sweep.elapsed() >= Duration::from_secs(RATE_LIMITER_SWEEP_INTERVAL_SECS) {
            Self::prune_stale(requests, cutoff);
            *last_sweep = now;
        }

        if !requests.contains_key(key) && requests.len() >= self.max_keys {
            // Opportunistic stale cleanup before eviction under cardinality pressure.
            Self::prune_stale(requests, cutoff);
            *last_sweep = now;

            if requests.len() >= self.max_keys {
                let evict_key = requests
                    .iter()
                    .min_by_key(|(_, timestamps)| timestamps.last().copied().unwrap_or(cutoff))
                    .map(|(k, _)| k.clone());
                if let Some(evict_key) = evict_key {
                    requests.remove(&evict_key);
                }
            }
        }

        let entry = requests.entry(key.to_owned()).or_default();
        entry.retain(|instant| *instant > cutoff);

        if entry.len() >= self.limit_per_window as usize {
            return false;
        }

        entry.push(now);
        true
    }
}

#[derive(Debug)]
pub struct GatewayRateLimiter {
    pair: SlidingWindowRateLimiter,
    webhook: SlidingWindowRateLimiter,
}

impl GatewayRateLimiter {
    fn new(pair_per_minute: u32, webhook_per_minute: u32, max_keys: usize) -> Self {
        let window = Duration::from_secs(RATE_LIMIT_WINDOW_SECS);
        Self {
            pair: SlidingWindowRateLimiter::new(pair_per_minute, window, max_keys),
            webhook: SlidingWindowRateLimiter::new(webhook_per_minute, window, max_keys),
        }
    }

    fn allow_pair(&self, key: &str) -> bool {
        self.pair.allow(key)
    }

    fn allow_webhook(&self, key: &str) -> bool {
        self.webhook.allow(key)
    }
}

#[derive(Debug)]
pub struct IdempotencyStore {
    ttl: Duration,
    max_keys: usize,
    keys: Mutex<HashMap<String, Instant>>,
}

impl IdempotencyStore {
    fn new(ttl: Duration, max_keys: usize) -> Self {
        Self {
            ttl,
            max_keys: max_keys.max(1),
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if this key is new and is now recorded.
    fn record_if_new(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut keys = self.keys.lock();

        keys.retain(|_, seen_at| now.duration_since(*seen_at) < self.ttl);

        if keys.contains_key(key) {
            return false;
        }

        if keys.len() >= self.max_keys {
            let evict_key = keys
                .iter()
                .min_by_key(|(_, seen_at)| *seen_at)
                .map(|(k, _)| k.clone());
            if let Some(evict_key) = evict_key {
                keys.remove(&evict_key);
            }
        }

        keys.insert(key.to_owned(), now);
        true
    }
}

fn parse_client_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim().trim_matches('"').trim();
    if value.is_empty() {
        return None;
    }

    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip);
    }

    if let Ok(addr) = value.parse::<SocketAddr>() {
        return Some(addr.ip());
    }

    let value = value.trim_matches(['[', ']']);
    value.parse::<IpAddr>().ok()
}

fn forwarded_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    if let Some(xff) = headers.get("X-Forwarded-For").and_then(|v| v.to_str().ok()) {
        for candidate in xff.split(',') {
            if let Some(ip) = parse_client_ip(candidate) {
                return Some(ip);
            }
        }
    }

    headers
        .get("X-Real-IP")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_client_ip)
}

pub(crate) fn client_key_from_request(
    peer_addr: Option<SocketAddr>,
    headers: &HeaderMap,
    trust_forwarded_headers: bool,
) -> String {
    if trust_forwarded_headers {
        if let Some(ip) = forwarded_client_ip(headers) {
            return ip.to_string();
        }
    }

    peer_addr
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn request_ip_from_request(
    peer_addr: Option<SocketAddr>,
    headers: &HeaderMap,
    trust_forwarded_headers: bool,
) -> Option<IpAddr> {
    if trust_forwarded_headers {
        if let Some(ip) = forwarded_client_ip(headers) {
            return Some(ip);
        }
    }

    peer_addr.map(|addr| addr.ip())
}

fn is_loopback_request(
    peer_addr: Option<SocketAddr>,
    headers: &HeaderMap,
    trust_forwarded_headers: bool,
) -> bool {
    request_ip_from_request(peer_addr, headers, trust_forwarded_headers)
        .is_some_and(|ip| ip.is_loopback())
}

fn normalize_max_keys(configured: usize, fallback: usize) -> usize {
    if configured == 0 {
        fallback.max(1)
    } else {
        configured
    }
}

/// Shared state for all axum handlers
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub temperature: f64,
    pub mem: Arc<dyn Memory>,
    pub auto_save: bool,
    /// SHA-256 hash of `X-Webhook-Secret` (hex-encoded), never plaintext.
    pub webhook_secret_hash: Option<Arc<str>>,
    pub pairing: Arc<PairingGuard>,
    pub trust_forwarded_headers: bool,
    pub rate_limiter: Arc<GatewayRateLimiter>,
    pub idempotency_store: Arc<IdempotencyStore>,
    /// Observability backend for metrics scraping
    pub observer: Arc<dyn crate::observability::Observer>,
    /// Registered tool specs (for web dashboard tools page)
    pub tools_registry: Arc<Vec<ToolSpec>>,
    /// Executable tools for agent loop (web chat)
    pub tools_registry_exec: Arc<Vec<Box<dyn Tool>>>,
    /// Multimodal config for image handling in web chat
    pub multimodal: crate::config::MultimodalConfig,
    /// Max tool iterations for agent loop
    pub max_tool_iterations: usize,
    /// Cost tracker (optional, for web dashboard cost page)
    pub cost_tracker: Option<Arc<CostTracker>>,
    /// SSE broadcast channel for real-time events
    pub event_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// Runtime trace file path for SSE backfill on connect
    pub runtime_trace_path: Option<std::path::PathBuf>,
}

/// Run the HTTP gateway using axum with proper HTTP/1.1 compliance.
#[allow(clippy::too_many_lines)]
pub async fn run_gateway(
    host: &str,
    port: u16,
    config: Config,
    external_pairing: Option<Arc<PairingGuard>>,
    external_event_tx: Option<tokio::sync::broadcast::Sender<serde_json::Value>>,
) -> Result<()> {
    // ── Security: refuse public bind without tunnel or explicit opt-in ──
    if is_public_bind(host) && config.tunnel.provider == "none" && !config.gateway.allow_public_bind
    {
        anyhow::bail!(
            "🛑 Refusing to bind to {host} — gateway would be reachable outside localhost\n\
             (for example from your local network, and potentially the internet\n\
             depending on your router/firewall setup).\n\
             Fix: use --host 127.0.0.1 (default), configure a tunnel, or set\n\
             [gateway] allow_public_bind = true in config.toml (NOT recommended)."
        );
    }
    let config_state = Arc::new(Mutex::new(config.clone()));

    // ── Hooks ──────────────────────────────────────────────────────
    let hooks = crate::hooks::create_runner_from_config(&config.hooks);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_port = listener.local_addr()?.port();

    let provider: Arc<dyn Provider> = Arc::from(providers::create_resilient_provider_with_options(
        config.default_provider.as_deref().unwrap_or("openrouter"),
        config.api_key.as_deref(),
        config.api_url.as_deref(),
        &config.reliability,
        &providers::ProviderRuntimeOptions {
            auth_profile_override: None,
            provider_api_url: config.api_url.clone(),
            provider_transport: config.effective_provider_transport(),
            zeroclaw_dir: config.config_path.parent().map(std::path::PathBuf::from),
            secrets_encrypt: config.secrets.encrypt,
            reasoning_enabled: config.runtime.reasoning_enabled,
            reasoning_level: config.effective_provider_reasoning_level(),
            custom_provider_api_mode: config.provider_api.map(|mode| mode.as_compatible_mode()),
            custom_provider_auth_header: config.effective_custom_provider_auth_header(),
            max_tokens_override: None,
            model_support_vision: config.model_support_vision,
            litellm_cache: config.effective_litellm_cache(),
            user_agent: config.effective_provider_user_agent(),
        },
    )?);
    let model = config
        .default_model
        .clone()
        .unwrap_or_else(|| "anthropic/claude-sonnet-4".into());
    let temperature = config.default_temperature;
    let mem: Arc<dyn Memory> = Arc::from(memory::create_memory_with_storage(
        &config.memory,
        Some(&config.storage.provider.config),
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(SecurityPolicy::from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));

    let tools_registry_exec: Arc<Vec<Box<dyn Tool>>> = Arc::new(
        crate::agent::tools_registry::build_tools_registry(
            &config,
            &security,
            runtime,
            Arc::clone(&mem),
            crate::agent::tools_registry::ToolsRegistryOptions::MINIMAL,
        )
        .await?,
    );
    let tools_registry: Arc<Vec<ToolSpec>> =
        Arc::new(tools_registry_exec.iter().map(|t| t.spec()).collect());
    let max_tool_iterations = config.agent.max_tool_iterations;
    let multimodal_config = config.multimodal.clone();

    // Cost tracker (optional)
    let cost_tracker = if config.cost.enabled {
        match CostTracker::new(config.cost.clone(), &config.workspace_dir) {
            Ok(ct) => Some(Arc::new(ct)),
            Err(e) => {
                tracing::warn!("Failed to initialize cost tracker: {e}");
                None
            }
        }
    } else {
        None
    };

    // SSE broadcast channel for real-time events — use shared channel when
    // provided by the daemon so channel listeners can broadcast to the same
    // SSE stream, otherwise create a local one for standalone gateway mode.
    let (event_tx, _event_rx) = match external_event_tx {
        Some(tx) => {
            let rx = tx.subscribe();
            (tx, rx)
        }
        None => tokio::sync::broadcast::channel::<serde_json::Value>(256),
    };
    // Extract webhook secret for authentication
    let webhook_secret_hash: Option<Arc<str>> =
        config.channels_config.webhook.as_ref().and_then(|webhook| {
            webhook.secret.as_ref().and_then(|raw_secret| {
                let trimmed_secret = raw_secret.trim();
                (!trimmed_secret.is_empty())
                    .then(|| Arc::<str>::from(hash_webhook_secret(trimmed_secret)))
            })
        });

    // ── Pairing guard ──────────────────────────────────────
    let pairing = external_pairing.unwrap_or_else(|| {
        Arc::new(PairingGuard::new(
            config.gateway.require_pairing,
            &config.gateway.paired_tokens,
            config.gateway.pairing_code.clone(),
        ))
    });

    // Load persisted device metadata from workspace state directory
    let meta_path = paired_devices_meta_path(&config.workspace_dir);
    if let Ok(json) = tokio::fs::read_to_string(&meta_path).await {
        if let Ok(meta_map) = serde_json::from_str::<std::collections::HashMap<String, crate::security::pairing::PairedDeviceMeta>>(&json) {
            pairing.load_meta_from_file(meta_map);
        }
    }

    // Clear the one-shot pairing code from persisted config so it is not reused.
    if config.gateway.pairing_code.is_some() {
        if let Ok(mut persisted) = Config::load_or_init().await {
            persisted.gateway.pairing_code = None;
            let _ = persisted.save().await;
        }
    }

    let rate_limit_max_keys = normalize_max_keys(
        config.gateway.rate_limit_max_keys,
        RATE_LIMIT_MAX_KEYS_DEFAULT,
    );
    let rate_limiter = Arc::new(GatewayRateLimiter::new(
        config.gateway.pair_rate_limit_per_minute,
        config.gateway.webhook_rate_limit_per_minute,
        rate_limit_max_keys,
    ));
    let idempotency_max_keys = normalize_max_keys(
        config.gateway.idempotency_max_keys,
        IDEMPOTENCY_MAX_KEYS_DEFAULT,
    );
    let idempotency_store = Arc::new(IdempotencyStore::new(
        Duration::from_secs(config.gateway.idempotency_ttl_secs.max(1)),
        idempotency_max_keys,
    ));

    // ── Tunnel ────────────────────────────────────────────────
    let tunnel = crate::tunnel::create_tunnel(&config.tunnel)?;

    if let Some(ref tun) = tunnel {
        match tun.start(host, actual_port).await {
            Ok(url) => {
                tracing::info!("🔗 Tunnel active: {url}");
            }
            Err(e) => {
                tracing::warn!("⚠️  Tunnel failed to start: {e}");
                tracing::warn!("Falling back to local-only mode.");
            }
        }
    }

    if let Some(code) = pairing.pairing_code() {
        tracing::info!("");
        tracing::info!("🔐 PAIRING REQUIRED — use this one-time code:");
        tracing::info!("   ┌──────────────┐");
        tracing::info!("      {code}");
        tracing::info!("   └──────────────┘");
        tracing::info!("Send: POST /pair with header X-Pairing-Code: {code}");
    } else if pairing.require_pairing() {
        tracing::info!("🔒 Pairing: ACTIVE (bearer token required)");
    } else {
        tracing::warn!("⚠️  Pairing: DISABLED (all requests accepted)");
    }

    crate::health::mark_component_ok("gateway");

    // Fire gateway start hook
    if let Some(ref hooks) = hooks {
        hooks.fire_gateway_start(host, actual_port).await;
    }

    // Wrap observer with broadcast capability for SSE
    // Use cost-tracking observer when cost tracking is enabled.
    // Wrap it in ObserverBridge so plugin hooks can observe a stable interface.
    let base_observer = crate::observability::create_observer_with_cost_tracking(
        &config.observability,
        cost_tracker.clone(),
        &config.cost,
    );
    let bridged_observer = crate::plugins::bridge::observer::ObserverBridge::new_box(base_observer);
    let broadcast_observer: Arc<dyn crate::observability::Observer> = Arc::new(
        sse::BroadcastObserver::new(Box::new(bridged_observer), event_tx.clone()),
    );

    let runtime_trace_path = {
        let mode =
            crate::observability::runtime_trace::storage_mode_from_config(&config.observability);
        if mode != crate::observability::runtime_trace::RuntimeTraceStorageMode::None {
            Some(crate::observability::runtime_trace::resolve_trace_path(
                &config.observability,
                &config.workspace_dir,
            ))
        } else {
            None
        }
    };

    let state = AppState {
        config: config_state,
        provider,
        model,
        temperature,
        mem,
        auto_save: config.memory.auto_save,
        webhook_secret_hash,
        pairing,
        trust_forwarded_headers: config.gateway.trust_forwarded_headers,
        rate_limiter,
        idempotency_store,
        observer: broadcast_observer,
        tools_registry,
        tools_registry_exec,
        multimodal: multimodal_config,
        max_tool_iterations,
        cost_tracker,
        event_tx,
        runtime_trace_path,
    };

    // Config PUT needs larger body limit (1MB)
    let config_put_router = Router::new()
        .route("/api/config", put(api::handle_api_config_put))
        .layer(RequestBodyLimitLayer::new(1_048_576));

    // The OpenAI-compatible endpoints use a larger body limit (512KB) because
    // chat histories can be much bigger than the default 64KB webhook limit.
    // They get their own nested router with a separate body limit layer.
    //
    // NOTE: The /v1/chat/completions handler routes through the full agent loop
    // (run_gateway_chat_with_tools) via openclaw_compat, giving OpenClaw callers
    // tools + memory support. The original simple-chat handler is preserved in
    // openai_compat.rs for reference.
    let openai_compat_routes = Router::new()
        .route(
            "/v1/chat/completions",
            post(openclaw_compat::handle_v1_chat_completions_with_tools),
        )
        .layer(RequestBodyLimitLayer::new(
            openai_compat::CHAT_COMPLETIONS_MAX_BODY_SIZE,
        ));

    // Build router with middleware
    let app = Router::new()
        // ── Existing routes ──
        .route("/health", get(handle_health))
        .route("/metrics", get(handle_metrics))
        .route("/pair", post(handle_pair))
        .route("/admin/paircode/new", post(handle_admin_paircode_new))
        .route("/webhook", get(handle_webhook_usage).post(handle_webhook))
        // ── OpenClaw migration: tools-enabled chat endpoint ──
        .route("/api/chat", post(openclaw_compat::handle_api_chat))
        // ── OpenAI-compatible endpoints ──
        .route("/v1/models", get(openai_compat::handle_v1_models))
        .merge(openai_compat_routes)
        // ── Web Dashboard API routes ──
        .route("/api/status", get(api::handle_api_status))
        .route("/api/config", get(api::handle_api_config_get))
        .route("/api/tools", get(api::handle_api_tools))
        .route("/api/cron", get(api::handle_api_cron_list))
        .route("/api/cron", post(api::handle_api_cron_add))
        .route("/api/cron/{id}", delete(api::handle_api_cron_delete))
        .route("/api/integrations", get(api::handle_api_integrations))
        .route(
            "/api/integrations/settings",
            get(api::handle_api_integrations_settings),
        )
        .route(
            "/api/integrations/{id}/credentials",
            put(api::handle_api_integrations_credentials_put),
        )
        .route(
            "/api/doctor",
            get(api::handle_api_doctor).post(api::handle_api_doctor),
        )
        .route("/api/memory", get(api::handle_api_memory_list))
        .route("/api/memory", post(api::handle_api_memory_store))
        .route("/api/memory/{key}", delete(api::handle_api_memory_delete))
        .route("/api/pairing/initiate", post(api::handle_api_pairing_initiate))
        .route("/api/pairing/devices", get(api::handle_api_pairing_devices))
        .route(
            "/api/pairing/devices/{id}",
            delete(api::handle_api_pairing_device_revoke),
        )
        .route("/api/cost", get(api::handle_api_cost))
        .route("/api/cli-tools", get(api::handle_api_cli_tools))
        .route("/api/skills", get(api::handle_api_skills))
        .route("/api/health", get(api::handle_api_health))
        .route("/api/workspace/files", get(api::handle_api_workspace_files))
        .route("/api/workspace/file", get(api::handle_api_workspace_file))
        .route("/api/node-control", post(handle_node_control))
        // ── SSE event stream ──
        .route("/api/events", get(sse::handle_sse_events))
        // ── WebSocket agent chat ──
        .route("/ws/chat", get(ws::handle_ws_chat))
        // ── Static assets (web dashboard) ──
        .route("/_app/{*path}", get(static_files::handle_static))
        // ── Config PUT with larger body limit ──
        .merge(config_put_router)
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_SIZE))
        .layer(middleware::from_fn(security_headers_middleware))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        // ── SPA fallback: non-API GET requests serve index.html ──
        .fallback(get(static_files::handle_spa_fallback));

    // Run the server
    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await;

    if let Some(ref hooks) = hooks {
        hooks.fire_gateway_stop().await;
    }

    serve_result?;

    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// AXUM HANDLERS
// ══════════════════════════════════════════════════════════════════════════════

/// GET /health — always public (no secrets leaked)
async fn handle_health(State(state): State<AppState>) -> impl IntoResponse {
    let body = serde_json::json!({
        "status": "ok",
        "paired": state.pairing.is_paired(),
        "require_pairing": state.pairing.require_pairing(),
        "runtime": crate::health::snapshot_json(),
    });
    Json(body)
}

/// Find the PrometheusObserver through wrapper layers (BroadcastObserver, ObserverBridge, MultiObserver).
fn find_prometheus_observer(
    obs: &dyn crate::observability::Observer,
) -> Option<&crate::observability::PrometheusObserver> {
    let any = obs.as_any();

    // Try direct downcast (if not wrapped)
    if let Some(prom) = any.downcast_ref::<crate::observability::PrometheusObserver>() {
        return Some(prom);
    }

    // Peel SharedPrometheusObserver (factory hands these out so every component
    // shares the singleton registry).
    if let Some(shared) = any.downcast_ref::<crate::observability::SharedPrometheusObserver>() {
        return Some(shared.inner());
    }

    // Peel BroadcastObserver (used by the gateway to fan events to SSE).
    if let Some(broadcast) = any.downcast_ref::<sse::BroadcastObserver>() {
        return find_prometheus_observer(broadcast.inner());
    }

    // Peel ObserverBridge (used by the plugin hook layer).
    if let Some(bridge) = any.downcast_ref::<crate::plugins::bridge::observer::ObserverBridge>() {
        return find_prometheus_observer(bridge.inner());
    }

    // Recurse through MultiObserver children so we descend through any further
    // wrapper layers (e.g. another `SharedPrometheusObserver`, nested
    // `MultiObserver`) — `MultiObserver::find_observer::<T>` only handles
    // direct downcasts and would miss the wrapped case.
    if let Some(multi) = any.downcast_ref::<crate::observability::MultiObserver>() {
        for child in multi.iter() {
            if let Some(prom) = find_prometheus_observer(child) {
                return Some(prom);
            }
        }
    }

    None
}

/// Prometheus content type for text exposition format.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// GET /metrics — Prometheus text exposition format
async fn handle_metrics(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if state.pairing.require_pairing() {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = auth.strip_prefix("Bearer ").unwrap_or("").trim();
        if !state.pairing.is_authenticated(token) {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
                String::from(
                    "# unauthorized: provide Authorization: Bearer <token> for /metrics\n",
                ),
            );
        }
    } else if !is_loopback_request(Some(peer_addr), &headers, state.trust_forwarded_headers) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
            String::from(
                "# metrics disabled for non-loopback clients when pairing is not required\n",
            ),
        );
    }

    let body = find_prometheus_observer(state.observer.as_ref())
        .map(|prom| prom.encode())
        .unwrap_or_else(|| {
            String::from(
                "# Prometheus backend not enabled. Set [observability] backend = \"prometheus\" in config.\n",
            )
        });

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        body,
    )
}

/// POST /pair — exchange one-time code for bearer token
#[axum::debug_handler]
async fn handle_pair(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let rate_key =
        client_key_from_request(Some(peer_addr), &headers, state.trust_forwarded_headers);
    if !state.rate_limiter.allow_pair(&rate_key) {
        tracing::warn!("/pair rate limit exceeded");
        let err = serde_json::json!({
            "error": "Too many pairing requests. Please retry later.",
            "retry_after": RATE_LIMIT_WINDOW_SECS,
        });
        return (StatusCode::TOO_MANY_REQUESTS, Json(err));
    }

    let code = headers
        .get("X-Pairing-Code")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match state.pairing.try_pair(code, &rate_key).await {
        Ok(Some(token)) => {
            tracing::info!("🔐 New client paired successfully");
            if let Err(err) = persist_pairing_tokens(state.config.clone(), &state.pairing).await {
                tracing::error!("🔐 Pairing succeeded but token persistence failed: {err:#}");
                let body = serde_json::json!({
                    "paired": true,
                    "persisted": false,
                    "token": token,
                    "message": "Paired for this process, but failed to persist token to config.toml. Check config path and write permissions.",
                });
                return (StatusCode::OK, Json(body));
            }

            // Also persist device metadata
            let workspace_dir = state.config.lock().workspace_dir.clone();
            if let Err(err) = persist_pairing_meta(&workspace_dir, &state.pairing).await {
                tracing::warn!("🔐 Failed to persist pairing metadata: {err}");
                // Don't fail the response — metadata persistence is secondary
            }

            let body = serde_json::json!({
                "paired": true,
                "persisted": true,
                "token": token,
                "message": "Save this token — use it as Authorization: Bearer <token>"
            });
            (StatusCode::OK, Json(body))
        }
        Ok(None) => {
            tracing::warn!("🔐 Pairing attempt with invalid code");
            let err = serde_json::json!({"error": "Invalid pairing code"});
            (StatusCode::FORBIDDEN, Json(err))
        }
        Err(lockout_secs) => {
            tracing::warn!(
                "🔐 Pairing locked out — too many failed attempts ({lockout_secs}s remaining)"
            );
            let err = serde_json::json!({
                "error": format!("Too many failed attempts. Try again in {lockout_secs}s."),
                "retry_after": lockout_secs
            });
            (StatusCode::TOO_MANY_REQUESTS, Json(err))
        }
    }
}

/// POST /admin/paircode/new — generate a new invite code without revoking existing tokens.
///
/// Localhost-only: rejects requests from non-loopback addresses so external
/// callers cannot mint codes without physical access to the machine.
async fn handle_admin_paircode_new(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if !peer_addr.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "localhost only"})),
        );
    }
    let code = state.pairing.generate_paircode();
    tracing::info!("🔐 New pairing invite code generated via /admin/paircode/new");
    (
        StatusCode::OK,
        Json(serde_json::json!({"pairing_code": code})),
    )
}

async fn persist_pairing_tokens(config: Arc<Mutex<Config>>, pairing: &PairingGuard) -> Result<()> {
    let paired_tokens = pairing.tokens();
    // This is needed because parking_lot's guard is not Send so we clone the inner
    // this should be removed once async mutexes are used everywhere
    let mut updated_cfg = { config.lock().clone() };
    updated_cfg.gateway.paired_tokens = paired_tokens;
    updated_cfg
        .save()
        .await
        .context("Failed to persist paired tokens to config.toml")?;

    // Keep shared runtime config in sync with persisted tokens.
    *config.lock() = updated_cfg;
    Ok(())
}

fn paired_devices_meta_path(workspace_dir: &std::path::Path) -> std::path::PathBuf {
    workspace_dir.join("state").join("paired_devices_meta.json")
}

pub async fn persist_pairing_meta(workspace_dir: &std::path::Path, pairing: &PairingGuard) -> Result<()> {
    let meta = pairing.device_meta_snapshot();
    let path = paired_devices_meta_path(workspace_dir);
    // Use a unique temp name so concurrent writers (e.g. an overlapping /pair
    // and revoke) don't clobber each other's temp file before the rename.
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec_pretty(&meta)?;
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    tokio::fs::write(&tmp, &json).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(())
}

/// Simple chat for webhook endpoint (no tools, for backward compatibility and testing).
async fn prepare_gateway_messages_for_provider(
    state: &AppState,
    message: &str,
) -> anyhow::Result<Vec<ChatMessage>> {
    let user_messages = vec![ChatMessage::user(message)];

    // Keep webhook/gateway prompts aligned with channel behavior by injecting
    // workspace-aware system context before model invocation.
    let system_prompt = {
        let config_guard = state.config.lock();
        crate::agent::prompt::build_system_prompt_with_mode(
            &config_guard,
            &[], // tools - empty for simple chat
            false,
            "gateway",
        )
    };

    let mut messages = Vec::with_capacity(1 + user_messages.len());
    messages.push(ChatMessage::system(system_prompt));
    messages.extend(user_messages);

    let (multimodal_config, provider_hint) = {
        let config = state.config.lock();
        (config.multimodal.clone(), config.default_provider.clone())
    };
    let prepared = crate::multimodal::prepare_messages_for_provider_with_provider_hint(
        &messages,
        &multimodal_config,
        provider_hint.as_deref(),
    )
    .await?;

    Ok(prepared.messages)
}

/// Simple chat for webhook endpoint (no tools, for backward compatibility and testing).
async fn run_gateway_chat_simple(state: &AppState, message: &str) -> anyhow::Result<String> {
    let prepared_messages = prepare_gateway_messages_for_provider(state, message).await?;

    state
        .provider
        .chat_with_history(&prepared_messages, &state.model, state.temperature)
        .await
}

/// Full-featured chat with tools for channel handlers.
pub(super) async fn run_gateway_chat_with_tools(
    state: &AppState,
    message: &str,
    session_id: Option<&str>,
) -> anyhow::Result<String> {
    let config = state.config.lock().clone();
    crate::agent::process_message_with_session(
        config,
        message,
        session_id,
        Some(Arc::clone(&state.observer)),
    )
    .await
}

fn gateway_outbound_leak_guard_snapshot(
    state: &AppState,
) -> crate::config::OutboundLeakGuardConfig {
    state.config.lock().security.outbound_leak_guard.clone()
}

fn sanitize_gateway_response(
    response: &str,
    tools: &[Box<dyn Tool>],
    leak_guard: &crate::config::OutboundLeakGuardConfig,
) -> String {
    match crate::channels::sanitize_channel_response(response, tools, leak_guard) {
        crate::channels::ChannelSanitizationResult::Sanitized(sanitized) => {
            if sanitized.is_empty() && !response.trim().is_empty() {
                "I encountered malformed tool-call output and could not produce a safe reply. Please try again."
                    .to_string()
            } else {
                sanitized
            }
        }
        crate::channels::ChannelSanitizationResult::Blocked { .. } => {
            "I blocked a draft response because it appeared to contain credential material. Please ask for a redacted summary."
                .to_string()
        }
    }
}

/// Webhook request body
#[derive(serde::Deserialize)]
pub struct WebhookBody {
    pub message: String,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct NodeControlRequest {
    pub method: String,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub capability: Option<String>,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

fn node_id_allowed(node_id: &str, allowed_node_ids: &[String]) -> bool {
    if allowed_node_ids.is_empty() {
        return true;
    }

    allowed_node_ids
        .iter()
        .any(|candidate| candidate == "*" || candidate == node_id)
}

/// POST /api/node-control — experimental node-control protocol scaffold.
///
/// Supported methods:
/// - `node.list`
/// - `node.describe`
/// - `node.invoke` (stubbed as not implemented)
async fn handle_node_control(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<NodeControlRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let node_control = { state.config.lock().gateway.node_control.clone() };
    if !node_control.enabled {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Node-control API is disabled"})),
        );
    }

    // Require at least one auth layer for non-loopback traffic:
    // 1) gateway pairing token, or
    // 2) node-control shared token.
    let has_node_control_token = node_control
        .auth_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if !state.pairing.require_pairing()
        && !has_node_control_token
        && !is_loopback_request(Some(peer_addr), &headers, state.trust_forwarded_headers)
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Unauthorized — enable gateway pairing or configure gateway.node_control.auth_token for non-local access"
            })),
        );
    }

    // ── Bearer auth (pairing) ──
    if state.pairing.require_pairing() {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = auth.strip_prefix("Bearer ").unwrap_or("");
        if !state.pairing.is_authenticated(token) {
            let err = serde_json::json!({
                "error": "Unauthorized — pair first via POST /pair, then send Authorization: Bearer <token>"
            });
            return (StatusCode::UNAUTHORIZED, Json(err));
        }
    }

    let Json(request) = match body {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!("Node-control JSON parse error: {e}");
            let err = serde_json::json!({
                "error": "Invalid JSON body for node-control request"
            });
            return (StatusCode::BAD_REQUEST, Json(err));
        }
    };

    // Optional second-factor shared token for node-control endpoints.
    if let Some(expected_token) = node_control
        .auth_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let provided_token = headers
            .get("X-Node-Control-Token")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .unwrap_or("");
        if !constant_time_eq(expected_token, provided_token) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Invalid X-Node-Control-Token"})),
            );
        }
    }

    let method = request.method.trim();
    match method {
        "node.list" => {
            let nodes = node_control
                .allowed_node_ids
                .iter()
                .map(|node_id| {
                    serde_json::json!({
                        "node_id": node_id,
                        "status": "unpaired",
                        "capabilities": []
                    })
                })
                .collect::<Vec<_>>();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "method": "node.list",
                    "nodes": nodes
                })),
            )
        }
        "node.describe" => {
            let Some(node_id) = request
                .node_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "node_id is required for node.describe"})),
                );
            };
            if !node_id_allowed(node_id, &node_control.allowed_node_ids) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "node_id is not allowed"})),
                );
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "method": "node.describe",
                    "node_id": node_id,
                    "description": {
                        "status": "stub",
                        "capabilities": [],
                        "message": "Node descriptor scaffold is enabled; runtime backend is not wired yet."
                    }
                })),
            )
        }
        "node.invoke" => {
            let Some(node_id) = request
                .node_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "node_id is required for node.invoke"})),
                );
            };
            if !node_id_allowed(node_id, &node_control.allowed_node_ids) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({"error": "node_id is not allowed"})),
                );
            }

            (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({
                    "ok": false,
                    "method": "node.invoke",
                    "node_id": node_id,
                    "capability": request.capability,
                    "arguments": request.arguments,
                    "error": "node.invoke backend is not implemented yet in this scaffold"
                })),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Unsupported method",
                "supported_methods": ["node.list", "node.describe", "node.invoke"]
            })),
        ),
    }
}

/// POST /webhook — main webhook endpoint
async fn handle_webhook_usage() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(serde_json::json!({
            "error": "Use POST /webhook with a JSON body: {\"message\":\"...\"}",
            "method": "POST",
            "path": "/webhook",
            "example": {
                "message": "Hello from webhook"
            }
        })),
    )
}

fn handle_webhook_streaming(
    state: AppState,
    prepared_messages: Vec<ChatMessage>,
    provider_label: String,
    model_label: String,
    started_at: Instant,
) -> Response {
    if !state.provider.supports_streaming() {
        let model_for_call = state.model.clone();
        let provider_label_for_call = provider_label.clone();
        let model_label_for_call = model_label.clone();
        let state_for_call = state.clone();
        let messages_for_call = prepared_messages.clone();

        let stream = futures_util::stream::once(async move {
            match state_for_call
                .provider
                .chat_with_history(
                    &messages_for_call,
                    &model_for_call,
                    state_for_call.temperature,
                )
                .await
            {
                Ok(response) => {
                    let leak_guard_cfg = gateway_outbound_leak_guard_snapshot(&state_for_call);
                    let safe_response = sanitize_gateway_response(
                        &response,
                        state_for_call.tools_registry_exec.as_ref(),
                        &leak_guard_cfg,
                    );
                    let duration = started_at.elapsed();
                    state_for_call.observer.record_event(
                        &crate::observability::ObserverEvent::LlmResponse {
                            provider: provider_label_for_call.clone(),
                            model: model_label_for_call.clone(),
                            duration,
                            success: true,
                            error_message: None,
                            input_tokens: None,
                            output_tokens: None,
                            cached_input_tokens: None,
                            channel: Some("gateway".into()),
                        },
                    );
                    state_for_call.observer.record_metric(
                        &crate::observability::traits::ObserverMetric::RequestLatency(duration),
                    );
                    state_for_call.observer.record_event(
                        &crate::observability::ObserverEvent::AgentEnd {
                            provider: provider_label_for_call,
                            model: model_label_for_call,
                            duration,
                            tokens_used: None,
                            cost_usd: None,
                        },
                    );

                    let payload = serde_json::json!({"response": safe_response, "model": state_for_call.model});
                    let mut output = format!("data: {payload}\n\n");
                    output.push_str("data: [DONE]\n\n");
                    Ok::<_, std::io::Error>(Bytes::from(output))
                }
                Err(e) => {
                    let duration = started_at.elapsed();
                    let sanitized = providers::sanitize_api_error(&e.to_string());

                    state_for_call.observer.record_event(
                        &crate::observability::ObserverEvent::LlmResponse {
                            provider: provider_label_for_call.clone(),
                            model: model_label_for_call.clone(),
                            duration,
                            success: false,
                            error_message: Some(sanitized.clone()),
                            input_tokens: None,
                            output_tokens: None,
                            cached_input_tokens: None,
                            channel: Some("gateway".into()),
                        },
                    );
                    state_for_call.observer.record_metric(
                        &crate::observability::traits::ObserverMetric::RequestLatency(duration),
                    );
                    state_for_call.observer.record_event(
                        &crate::observability::ObserverEvent::Error {
                            component: "gateway".to_string(),
                            message: sanitized.clone(),
                        },
                    );
                    state_for_call.observer.record_event(
                        &crate::observability::ObserverEvent::AgentEnd {
                            provider: provider_label_for_call,
                            model: model_label_for_call,
                            duration,
                            tokens_used: None,
                            cost_usd: None,
                        },
                    );

                    tracing::error!("Webhook provider error: {}", sanitized);
                    let mut output = format!(
                        "data: {}\n\n",
                        serde_json::json!({"error": "LLM request failed"})
                    );
                    output.push_str("data: [DONE]\n\n");
                    Ok(Bytes::from(output))
                }
            }
        });

        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(stream))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let provider_stream = state.provider.stream_chat_with_history(
        &prepared_messages,
        &state.model,
        state.temperature,
        crate::providers::traits::StreamOptions::new(true),
    );

    let state_for_stream = state.clone();
    let provider_label_for_stream = provider_label.clone();
    let model_label_for_stream = model_label.clone();
    let mut stream_failed = false;

    let sse_stream = provider_stream.map(move |result| match result {
        Ok(chunk) if chunk.is_final => {
            if !stream_failed {
                let duration = started_at.elapsed();
                state_for_stream.observer.record_event(
                    &crate::observability::ObserverEvent::LlmResponse {
                        provider: provider_label_for_stream.clone(),
                        model: model_label_for_stream.clone(),
                        duration,
                        success: true,
                        error_message: None,
                        input_tokens: None,
                        output_tokens: None,
                        cached_input_tokens: None,
                        channel: Some("gateway".into()),
                    },
                );
                state_for_stream.observer.record_metric(
                    &crate::observability::traits::ObserverMetric::RequestLatency(duration),
                );
                state_for_stream.observer.record_event(
                    &crate::observability::ObserverEvent::AgentEnd {
                        provider: provider_label_for_stream.clone(),
                        model: model_label_for_stream.clone(),
                        duration,
                        tokens_used: None,
                        cost_usd: None,
                    },
                );
            }
            Ok::<_, std::io::Error>(Bytes::from("data: [DONE]\n\n"))
        }
        Ok(chunk) => {
            if chunk.delta.is_empty() {
                return Ok(Bytes::new());
            }
            let payload = serde_json::json!({
                "delta": chunk.delta,
                "model": model_label_for_stream
            });
            Ok(Bytes::from(format!("data: {payload}\n\n")))
        }
        Err(e) => {
            stream_failed = true;
            let duration = started_at.elapsed();
            let sanitized = providers::sanitize_api_error(&e.to_string());

            state_for_stream.observer.record_event(
                &crate::observability::ObserverEvent::LlmResponse {
                    provider: provider_label_for_stream.clone(),
                    model: model_label_for_stream.clone(),
                    duration,
                    success: false,
                    error_message: Some(sanitized.clone()),
                    input_tokens: None,
                    output_tokens: None,
                    cached_input_tokens: None,
                    channel: Some("gateway".into()),
                },
            );
            state_for_stream.observer.record_metric(
                &crate::observability::traits::ObserverMetric::RequestLatency(duration),
            );
            state_for_stream
                .observer
                .record_event(&crate::observability::ObserverEvent::Error {
                    component: "gateway".to_string(),
                    message: sanitized.clone(),
                });
            state_for_stream.observer.record_event(
                &crate::observability::ObserverEvent::AgentEnd {
                    provider: provider_label_for_stream.clone(),
                    model: model_label_for_stream.clone(),
                    duration,
                    tokens_used: None,
                    cost_usd: None,
                },
            );

            tracing::error!("Webhook streaming provider error: {}", sanitized);
            let output = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                serde_json::json!({"error": "LLM request failed"})
            );
            Ok(Bytes::from(output))
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(sse_stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// POST /webhook — main webhook endpoint
async fn handle_webhook(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<WebhookBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let rate_key =
        client_key_from_request(Some(peer_addr), &headers, state.trust_forwarded_headers);
    if !state.rate_limiter.allow_webhook(&rate_key) {
        tracing::warn!("/webhook rate limit exceeded");
        let err = serde_json::json!({
            "error": "Too many webhook requests. Please retry later.",
            "retry_after": RATE_LIMIT_WINDOW_SECS,
        });
        return (StatusCode::TOO_MANY_REQUESTS, Json(err)).into_response();
    }

    // Require at least one auth layer for non-loopback traffic.
    if !state.pairing.require_pairing()
        && state.webhook_secret_hash.is_none()
        && !is_loopback_request(Some(peer_addr), &headers, state.trust_forwarded_headers)
    {
        tracing::warn!(
            "Webhook: rejected unauthenticated non-loopback request (pairing disabled and no webhook secret configured)"
        );
        let err = serde_json::json!({
            "error": "Unauthorized — configure pairing or X-Webhook-Secret for non-local webhook access"
        });
        return (StatusCode::UNAUTHORIZED, Json(err)).into_response();
    }

    // ── Bearer token auth (pairing) ──
    if state.pairing.require_pairing() {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = auth.strip_prefix("Bearer ").unwrap_or("");
        if !state.pairing.is_authenticated(token) {
            tracing::warn!("Webhook: rejected — not paired / invalid bearer token");
            let err = serde_json::json!({
                "error": "Unauthorized — pair first via POST /pair, then send Authorization: Bearer <token>"
            });
            return (StatusCode::UNAUTHORIZED, Json(err)).into_response();
        }
    }

    // ── Webhook secret auth (optional, additional layer) ──
    if let Some(ref secret_hash) = state.webhook_secret_hash {
        let header_hash = headers
            .get("X-Webhook-Secret")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(hash_webhook_secret);
        match header_hash {
            Some(val) if constant_time_eq(&val, secret_hash.as_ref()) => {}
            _ => {
                tracing::warn!("Webhook: rejected request — invalid or missing X-Webhook-Secret");
                let err = serde_json::json!({"error": "Unauthorized — invalid or missing X-Webhook-Secret header"});
                return (StatusCode::UNAUTHORIZED, Json(err)).into_response();
            }
        }
    }

    // ── Parse body ──
    let Json(webhook_body) = match body {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Webhook JSON parse error: {e}");
            let err = serde_json::json!({
                "error": "Invalid JSON body. Expected: {\"message\": \"...\"}"
            });
            return (StatusCode::BAD_REQUEST, Json(err)).into_response();
        }
    };

    // ── Idempotency (optional) ──
    if let Some(idempotency_key) = headers
        .get("X-Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !state.idempotency_store.record_if_new(idempotency_key) {
            tracing::info!("Webhook duplicate ignored (idempotency key: {idempotency_key})");
            let body = serde_json::json!({
                "status": "duplicate",
                "idempotent": true,
                "message": "Request already processed for this idempotency key"
            });
            return (StatusCode::OK, Json(body)).into_response();
        }
    }

    let message = webhook_body.message.trim();
    let webhook_session_id = webhook_body
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if message.is_empty() {
        let err = serde_json::json!({
            "error": "The `message` field is required and must be a non-empty string."
        });
        return (StatusCode::BAD_REQUEST, Json(err)).into_response();
    }

    if state.auto_save {
        let key = webhook_memory_key();
        let _ = state
            .mem
            .store(
                &key,
                message,
                MemoryCategory::Conversation,
                webhook_session_id,
            )
            .await;
    }

    let provider_label = state
        .config
        .lock()
        .default_provider
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let model_label = state.model.clone();
    let started_at = Instant::now();

    state
        .observer
        .record_event(&crate::observability::ObserverEvent::AgentStart {
            provider: provider_label.clone(),
            model: model_label.clone(),
        });
    state
        .observer
        .record_event(&crate::observability::ObserverEvent::LlmRequest {
            provider: provider_label.clone(),
            model: model_label.clone(),
            messages_count: 1,
        });

    if webhook_body.stream.unwrap_or(false) {
        let prepared_messages = match prepare_gateway_messages_for_provider(&state, message).await {
            Ok(messages) => messages,
            Err(e) => {
                let duration = started_at.elapsed();
                let sanitized = providers::sanitize_api_error(&e.to_string());
                state
                    .observer
                    .record_event(&crate::observability::ObserverEvent::LlmResponse {
                        provider: provider_label.clone(),
                        model: model_label.clone(),
                        duration,
                        success: false,
                        error_message: Some(sanitized.clone()),
                        input_tokens: None,
                        output_tokens: None,
                        cached_input_tokens: None,
                        channel: Some("gateway".into()),
                    });
                state.observer.record_metric(
                    &crate::observability::traits::ObserverMetric::RequestLatency(duration),
                );
                state
                    .observer
                    .record_event(&crate::observability::ObserverEvent::Error {
                        component: "gateway".to_string(),
                        message: sanitized.clone(),
                    });
                state
                    .observer
                    .record_event(&crate::observability::ObserverEvent::AgentEnd {
                        provider: provider_label,
                        model: model_label,
                        duration,
                        tokens_used: None,
                        cost_usd: None,
                    });

                tracing::error!("Webhook streaming setup failed: {}", sanitized);
                let err = serde_json::json!({"error": "LLM request failed"});
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(err)).into_response();
            }
        };

        return handle_webhook_streaming(
            state,
            prepared_messages,
            provider_label,
            model_label,
            started_at,
        );
    }

    match run_gateway_chat_simple(&state, message).await {
        Ok(response) => {
            let leak_guard_cfg = gateway_outbound_leak_guard_snapshot(&state);
            let safe_response = sanitize_gateway_response(
                &response,
                state.tools_registry_exec.as_ref(),
                &leak_guard_cfg,
            );
            let duration = started_at.elapsed();
            state
                .observer
                .record_event(&crate::observability::ObserverEvent::LlmResponse {
                    provider: provider_label.clone(),
                    model: model_label.clone(),
                    duration,
                    success: true,
                    error_message: None,
                    input_tokens: None,
                    output_tokens: None,
                    cached_input_tokens: None,
                    channel: Some("gateway".into()),
                });
            state.observer.record_metric(
                &crate::observability::traits::ObserverMetric::RequestLatency(duration),
            );
            state
                .observer
                .record_event(&crate::observability::ObserverEvent::AgentEnd {
                    provider: provider_label,
                    model: model_label,
                    duration,
                    tokens_used: None,
                    cost_usd: None,
                });

            let body = serde_json::json!({"response": safe_response, "model": state.model});
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => {
            let duration = started_at.elapsed();
            let sanitized = providers::sanitize_api_error(&e.to_string());

            state
                .observer
                .record_event(&crate::observability::ObserverEvent::LlmResponse {
                    provider: provider_label.clone(),
                    model: model_label.clone(),
                    duration,
                    success: false,
                    error_message: Some(sanitized.clone()),
                    input_tokens: None,
                    output_tokens: None,
                    cached_input_tokens: None,
                    channel: Some("gateway".into()),
                });
            state.observer.record_metric(
                &crate::observability::traits::ObserverMetric::RequestLatency(duration),
            );
            state
                .observer
                .record_event(&crate::observability::ObserverEvent::Error {
                    component: "gateway".to_string(),
                    message: sanitized.clone(),
                });
            state
                .observer
                .record_event(&crate::observability::ObserverEvent::AgentEnd {
                    provider: provider_label,
                    model: model_label,
                    duration,
                    tokens_used: None,
                    cost_usd: None,
                });

            tracing::error!("Webhook provider error: {}", sanitized);
            let err = serde_json::json!({"error": "LLM request failed"});
            (StatusCode::INTERNAL_SERVER_ERROR, Json(err)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Memory, MemoryCategory, MemoryEntry};
    use crate::providers::Provider;
    use async_trait::async_trait;
    use axum::http::HeaderValue;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Generate a random hex secret at runtime to avoid hard-coded cryptographic values.
    fn generate_test_secret() -> String {
        let bytes: [u8; 32] = rand::random();
        hex::encode(bytes)
    }

    #[test]
    fn security_body_limit_is_64kb() {
        assert_eq!(MAX_BODY_SIZE, 65_536);
    }

    #[test]
    fn security_timeout_is_30_seconds() {
        assert_eq!(REQUEST_TIMEOUT_SECS, 30);
    }

    #[test]
    fn webhook_body_requires_message_field() {
        let valid = r#"{"message": "hello"}"#;
        let parsed: Result<WebhookBody, _> = serde_json::from_str(valid);
        assert!(parsed.is_ok());
        let parsed = parsed.unwrap();
        assert_eq!(parsed.message, "hello");
        assert_eq!(parsed.stream, None);

        let stream_enabled = r#"{"message": "hello", "stream": true}"#;
        let parsed: Result<WebhookBody, _> = serde_json::from_str(stream_enabled);
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap().stream, Some(true));

        let missing = r#"{"other": "field"}"#;
        let parsed: Result<WebhookBody, _> = serde_json::from_str(missing);
        assert!(parsed.is_err());
    }

    #[tokio::test]
    async fn webhook_get_usage_returns_explicit_method_hint() {
        let response = handle_webhook_usage().await.into_response();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        let payload = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(parsed["method"], "POST");
        assert_eq!(parsed["path"], "/webhook");
        assert_eq!(parsed["example"]["message"], "Hello from webhook");
    }

    #[test]
    fn node_id_allowed_with_empty_allowlist_accepts_any() {
        assert!(node_id_allowed("node-a", &[]));
    }

    #[test]
    fn node_id_allowed_respects_allowlist() {
        let allow = vec!["node-1".to_string(), "node-2".to_string()];
        assert!(node_id_allowed("node-1", &allow));
        assert!(!node_id_allowed("node-9", &allow));
    }

    #[test]
    fn app_state_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<AppState>();
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_hint_when_prometheus_is_disabled() {
        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_metrics(State(state), test_connect_info(), HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some(PROMETHEUS_CONTENT_TYPE)
        );

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("Prometheus backend not enabled"));
    }

    #[tokio::test]
    async fn metrics_endpoint_renders_prometheus_output() {
        let prom = Arc::new(
            crate::observability::PrometheusObserver::new()
                .expect("prometheus observer should initialize in tests"),
        );
        crate::observability::Observer::record_event(
            prom.as_ref(),
            &crate::observability::ObserverEvent::HeartbeatTick,
        );

        let observer: Arc<dyn crate::observability::Observer> = prom;
        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer,
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_metrics(State(state), test_connect_info(), HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("zeroclaw_heartbeat_ticks_total 1"));
    }

    #[tokio::test]
    async fn metrics_endpoint_unwraps_production_observer_chain() {
        // Mirrors the wrapper stack built by `gateway::start`:
        //   BroadcastObserver { ObserverBridge { MultiObserver { [PrometheusObserver, NoopObserver] } } }
        // Regression guard: `find_prometheus_observer` must descend through every layer or `/metrics`
        // silently returns the "not enabled" stub even though Prometheus is configured.
        let prom = crate::observability::PrometheusObserver::new()
            .expect("prometheus observer should initialize in tests");
        crate::observability::Observer::record_event(
            &prom,
            &crate::observability::ObserverEvent::HeartbeatTick,
        );
        let multi = crate::observability::MultiObserver::new(vec![
            Box::new(prom),
            Box::new(crate::observability::NoopObserver),
        ]);
        let bridged = crate::plugins::bridge::observer::ObserverBridge::new_box(Box::new(multi));
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(16);
        let observer: Arc<dyn crate::observability::Observer> = Arc::new(
            sse::BroadcastObserver::new(Box::new(bridged), event_tx.clone()),
        );

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer,
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx,
            runtime_trace_path: None,
        };

        let response = handle_metrics(State(state), test_connect_info(), HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("zeroclaw_heartbeat_ticks_total 1"),
            "expected Prometheus output through wrapped observer chain, got:\n{text}"
        );
        assert!(
            !text.contains("Prometheus backend not enabled"),
            "wrapper chain should not fall through to the disabled stub"
        );
    }

    /// End-to-end test of the singleton: build TWO independent factory chains
    /// (mirroring the real daemon: gateway + heartbeat worker), record events
    /// on the heartbeat-style chain only, and verify they show up in the
    /// gateway's `/metrics` output.
    ///
    /// If this fails, the singleton is not actually being shared across the
    /// process and `/metrics` will look frozen at zero from the dashboard.
    #[tokio::test]
    async fn metrics_endpoint_sees_events_recorded_by_other_factory_chains() {
        use std::sync::Arc;
        let observability = crate::config::ObservabilityConfig {
            backend: "prometheus".into(),
            ..crate::config::ObservabilityConfig::default()
        };
        let cost_config = crate::config::schema::CostConfig::default();

        // ── Heartbeat-worker style chain (no SSE wrapper). Mirrors `daemon::run_heartbeat_worker`.
        let heartbeat_observer: Arc<dyn crate::observability::Observer> =
            Arc::from(crate::observability::create_observer_with_cost_tracking(
                &observability,
                None,
                &cost_config,
            ));

        // ── Gateway-style chain (Broadcast + Bridge wrappers). Mirrors `gateway::run_gateway`.
        let gateway_base = crate::observability::create_observer_with_cost_tracking(
            &observability,
            None,
            &cost_config,
        );
        let bridged = crate::plugins::bridge::observer::ObserverBridge::new_box(gateway_base);
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(16);
        let gateway_observer: Arc<dyn crate::observability::Observer> = Arc::new(
            sse::BroadcastObserver::new(Box::new(bridged), event_tx.clone()),
        );

        // Record on the heartbeat chain only.
        for _ in 0..3 {
            heartbeat_observer.record_event(&crate::observability::ObserverEvent::HeartbeatTick);
        }

        // Read /metrics through the gateway chain — must see the heartbeat events.
        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: gateway_observer,
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx,
            runtime_trace_path: None,
        };

        let response = handle_metrics(State(state), test_connect_info(), HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let line = text
            .lines()
            .find(|l| l.starts_with("zeroclaw_heartbeat_ticks_total "))
            .unwrap_or_else(|| panic!("counter line missing in:\n{text}"));
        let value: u64 = line
            .split_whitespace()
            .nth(1)
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("counter value did not parse: {line}"));
        // Other tests in the same binary may have already incremented the
        // process-wide singleton, so assert ≥ 3 (the events written above)
        // rather than == 3.
        assert!(
            value >= 3,
            "gateway /metrics did not see heartbeat events from the other factory chain — \
             expected counter ≥ 3, got {value}. Singleton is not shared."
        );
    }

    /// Same as `metrics_endpoint_renders_through_real_factory_chain` but with
    /// cost tracking enabled, which wraps the prometheus observer in a
    /// `MultiObserver` alongside `CostObserver`. Catches the case where
    /// `MultiObserver::find_observer::<PrometheusObserver>` cannot peek through
    /// the `SharedPrometheusObserver` wrapper.
    #[tokio::test]
    async fn metrics_endpoint_renders_through_real_factory_chain_with_cost_tracking() {
        use std::sync::Arc;
        let observability = crate::config::ObservabilityConfig {
            backend: "prometheus".into(),
            ..crate::config::ObservabilityConfig::default()
        };
        let cost_config = crate::config::schema::CostConfig {
            enabled: true,
            ..crate::config::schema::CostConfig::default()
        };
        let tmp = tempfile::tempdir().expect("tempdir for cost storage");
        let cost_tracker = Arc::new(
            crate::cost::CostTracker::new(cost_config.clone(), tmp.path())
                .expect("CostTracker::new should succeed for tests"),
        );
        let base = crate::observability::create_observer_with_cost_tracking(
            &observability,
            Some(Arc::clone(&cost_tracker)),
            &cost_config,
        );
        crate::observability::Observer::record_event(
            base.as_ref(),
            &crate::observability::ObserverEvent::HeartbeatTick,
        );

        let bridged = crate::plugins::bridge::observer::ObserverBridge::new_box(base);
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(16);
        let observer: Arc<dyn crate::observability::Observer> = Arc::new(
            sse::BroadcastObserver::new(Box::new(bridged), event_tx.clone()),
        );

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer,
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: Some(cost_tracker),
            event_tx,
            runtime_trace_path: None,
        };

        let response = handle_metrics(State(state), test_connect_info(), HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("zeroclaw_heartbeat_ticks_total"),
            "expected heartbeat metric inside MultiObserver chain, got:\n{text}"
        );
        assert!(
            !text.contains("Prometheus backend not enabled"),
            "factory chain with cost tracking must not fall through to the disabled stub:\n{text}"
        );
    }

    /// Builds the production observer chain by going through the *real* factory
    /// (`create_observer_with_cost_tracking`), so it covers `SharedPrometheusObserver`
    /// — the wrapper the factory now hands out so every component shares one
    /// `Registry`. Regression guard for the dashboard metrics tab.
    #[tokio::test]
    async fn metrics_endpoint_renders_through_real_factory_chain() {
        let observability = crate::config::ObservabilityConfig {
            backend: "prometheus".into(),
            ..crate::config::ObservabilityConfig::default()
        };
        let cost_config = crate::config::schema::CostConfig::default();
        let base = crate::observability::create_observer_with_cost_tracking(
            &observability,
            None,
            &cost_config,
        );
        // Drive a heartbeat tick through the factory-built chain so the singleton
        // registry has something to expose.
        crate::observability::Observer::record_event(
            base.as_ref(),
            &crate::observability::ObserverEvent::HeartbeatTick,
        );

        let bridged = crate::plugins::bridge::observer::ObserverBridge::new_box(base);
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(16);
        let observer: Arc<dyn crate::observability::Observer> = Arc::new(
            sse::BroadcastObserver::new(Box::new(bridged), event_tx.clone()),
        );

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer,
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx,
            runtime_trace_path: None,
        };

        let response = handle_metrics(State(state), test_connect_info(), HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("zeroclaw_heartbeat_ticks_total"),
            "expected the heartbeat metric line in factory-built chain output, got:\n{text}"
        );
        assert!(
            !text.contains("Prometheus backend not enabled"),
            "factory-built chain must not fall through to the disabled stub:\n{text}"
        );
    }

    #[tokio::test]
    async fn metrics_endpoint_rejects_public_clients_when_pairing_is_disabled() {
        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_metrics(State(state), test_public_connect_info(), HeaderMap::new())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("non-loopback"));
    }

    #[tokio::test]
    async fn metrics_endpoint_requires_bearer_token_when_pairing_is_enabled() {
        let paired_token = "zc_test_token".to_string();
        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider: Arc::new(MockProvider::default()),
            model: "test-model".into(),
            temperature: 0.0,
            mem: Arc::new(MockMemory),
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(
                true,
                std::slice::from_ref(&paired_token),
                None,
            )),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let unauthorized =
            handle_metrics(State(state.clone()), test_connect_info(), HeaderMap::new())
                .await
                .into_response();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {paired_token}")).unwrap(),
        );
        let authorized = handle_metrics(State(state), test_connect_info(), headers)
            .await
            .into_response();
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    #[test]
    fn gateway_rate_limiter_blocks_after_limit() {
        let limiter = GatewayRateLimiter::new(2, 2, 100);
        assert!(limiter.allow_pair("127.0.0.1"));
        assert!(limiter.allow_pair("127.0.0.1"));
        assert!(!limiter.allow_pair("127.0.0.1"));
    }

    #[test]
    fn rate_limiter_sweep_removes_stale_entries() {
        let limiter = SlidingWindowRateLimiter::new(10, Duration::from_secs(60), 100);
        // Add entries for multiple IPs
        assert!(limiter.allow("ip-1"));
        assert!(limiter.allow("ip-2"));
        assert!(limiter.allow("ip-3"));

        {
            let guard = limiter.requests.lock();
            assert_eq!(guard.0.len(), 3);
        }

        // Force a sweep by backdating last_sweep
        {
            let mut guard = limiter.requests.lock();
            guard.1 = Instant::now()
                .checked_sub(Duration::from_secs(RATE_LIMITER_SWEEP_INTERVAL_SECS + 1))
                .unwrap();
            // Clear timestamps for ip-2 and ip-3 to simulate stale entries
            guard.0.get_mut("ip-2").unwrap().clear();
            guard.0.get_mut("ip-3").unwrap().clear();
        }

        // Next allow() call should trigger sweep and remove stale entries
        assert!(limiter.allow("ip-1"));

        {
            let guard = limiter.requests.lock();
            assert_eq!(guard.0.len(), 1, "Stale entries should have been swept");
            assert!(guard.0.contains_key("ip-1"));
        }
    }

    #[test]
    fn rate_limiter_zero_limit_always_allows() {
        let limiter = SlidingWindowRateLimiter::new(0, Duration::from_secs(60), 10);
        for _ in 0..100 {
            assert!(limiter.allow("any-key"));
        }
    }

    #[test]
    fn idempotency_store_rejects_duplicate_key() {
        let store = IdempotencyStore::new(Duration::from_secs(30), 10);
        assert!(store.record_if_new("req-1"));
        assert!(!store.record_if_new("req-1"));
        assert!(store.record_if_new("req-2"));
    }

    #[test]
    fn rate_limiter_bounded_cardinality_evicts_oldest_key() {
        let limiter = SlidingWindowRateLimiter::new(5, Duration::from_secs(60), 2);
        assert!(limiter.allow("ip-1"));
        assert!(limiter.allow("ip-2"));
        assert!(limiter.allow("ip-3"));

        let guard = limiter.requests.lock();
        assert_eq!(guard.0.len(), 2);
        assert!(guard.0.contains_key("ip-2"));
        assert!(guard.0.contains_key("ip-3"));
    }

    #[test]
    fn idempotency_store_bounded_cardinality_evicts_oldest_key() {
        let store = IdempotencyStore::new(Duration::from_secs(300), 2);
        assert!(store.record_if_new("k1"));
        std::thread::sleep(Duration::from_millis(2));
        assert!(store.record_if_new("k2"));
        std::thread::sleep(Duration::from_millis(2));
        assert!(store.record_if_new("k3"));

        let keys = store.keys.lock();
        assert_eq!(keys.len(), 2);
        assert!(!keys.contains_key("k1"));
        assert!(keys.contains_key("k2"));
        assert!(keys.contains_key("k3"));
    }

    #[test]
    fn client_key_defaults_to_peer_addr_when_untrusted_proxy_mode() {
        let peer = SocketAddr::from(([10, 0, 0, 5], 42617));
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_static("198.51.100.10, 203.0.113.11"),
        );

        let key = client_key_from_request(Some(peer), &headers, false);
        assert_eq!(key, "10.0.0.5");
    }

    #[test]
    fn client_key_uses_forwarded_ip_only_in_trusted_proxy_mode() {
        let peer = SocketAddr::from(([10, 0, 0, 5], 42617));
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_static("198.51.100.10, 203.0.113.11"),
        );

        let key = client_key_from_request(Some(peer), &headers, true);
        assert_eq!(key, "198.51.100.10");
    }

    #[test]
    fn client_key_falls_back_to_peer_when_forwarded_header_invalid() {
        let peer = SocketAddr::from(([10, 0, 0, 5], 42617));
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-For", HeaderValue::from_static("garbage-value"));

        let key = client_key_from_request(Some(peer), &headers, true);
        assert_eq!(key, "10.0.0.5");
    }

    #[test]
    fn is_loopback_request_uses_peer_addr_when_untrusted_proxy_mode() {
        let peer = SocketAddr::from(([203, 0, 113, 10], 42617));
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-For", HeaderValue::from_static("127.0.0.1"));

        assert!(!is_loopback_request(Some(peer), &headers, false));
    }

    #[test]
    fn is_loopback_request_uses_forwarded_ip_in_trusted_proxy_mode() {
        let peer = SocketAddr::from(([203, 0, 113, 10], 42617));
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-For", HeaderValue::from_static("127.0.0.1"));

        assert!(is_loopback_request(Some(peer), &headers, true));
    }

    #[test]
    fn is_loopback_request_falls_back_to_peer_when_forwarded_invalid() {
        let peer = SocketAddr::from(([203, 0, 113, 10], 42617));
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-For", HeaderValue::from_static("not-an-ip"));

        assert!(!is_loopback_request(Some(peer), &headers, true));
    }

    #[test]
    fn normalize_max_keys_uses_fallback_for_zero() {
        assert_eq!(normalize_max_keys(0, 10_000), 10_000);
        assert_eq!(normalize_max_keys(0, 0), 1);
    }

    #[test]
    fn normalize_max_keys_preserves_nonzero_values() {
        assert_eq!(normalize_max_keys(2_048, 10_000), 2_048);
        assert_eq!(normalize_max_keys(1, 10_000), 1);
    }

    #[tokio::test]
    async fn persist_pairing_tokens_writes_config_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        let workspace_path = temp.path().join("workspace");

        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.workspace_dir = workspace_path;
        config.save().await.unwrap();

        let guard = PairingGuard::new(true, &[], None);
        let code = guard.pairing_code().unwrap();
        let token = guard.try_pair(&code, "test_client").await.unwrap().unwrap();
        assert!(guard.is_authenticated(&token));

        let shared_config = Arc::new(Mutex::new(config));
        persist_pairing_tokens(shared_config.clone(), &guard)
            .await
            .unwrap();

        let saved = tokio::fs::read_to_string(config_path).await.unwrap();
        let parsed: Config = toml::from_str(&saved).unwrap();
        assert_eq!(parsed.gateway.paired_tokens.len(), 1);
        let persisted = &parsed.gateway.paired_tokens[0];
        assert!(crate::security::SecretStore::is_encrypted(persisted));
        let store = crate::security::SecretStore::new(temp.path(), true);
        let decrypted = store.decrypt(persisted).unwrap();
        assert_eq!(decrypted.len(), 64);
        assert!(decrypted.chars().all(|c| c.is_ascii_hexdigit()));

        let in_memory = shared_config.lock();
        assert_eq!(in_memory.gateway.paired_tokens.len(), 1);
        assert_eq!(&in_memory.gateway.paired_tokens[0], &decrypted);
    }

    #[test]
    fn webhook_memory_key_is_unique() {
        let key1 = webhook_memory_key();
        let key2 = webhook_memory_key();

        assert!(key1.starts_with("webhook_msg_"));
        assert!(key2.starts_with("webhook_msg_"));
        assert_ne!(key1, key2);
    }

    struct MockBrowserTool;

    #[async_trait]
    impl Tool for MockBrowserTool {
        fn name(&self) -> &str {
            "browser"
        }

        fn description(&self) -> &str {
            "Mock browser tool"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string" }
                },
                "required": ["action"]
            })
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

    #[test]
    fn sanitize_gateway_response_removes_tool_call_tags() {
        let input = r#"Before
<tool_call>
{"name":"browser","arguments":{"action":"screenshot"}}
</tool_call>
After"#;

        let leak_guard = crate::config::OutboundLeakGuardConfig::default();
        let result = sanitize_gateway_response(input, &[], &leak_guard);
        let normalized = result
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(normalized, "Before\nAfter");
        assert!(!result.contains("<tool_call>"));
        assert!(!result.contains("\"name\":\"browser\""));
    }

    #[test]
    fn sanitize_gateway_response_removes_isolated_tool_json_artifacts() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(MockBrowserTool)];
        let input = r#"{"name":"browser","parameters":{"action":"screenshot"}}
{"result":{"status":"captured"}}
Screenshot captured successfully."#;

        let leak_guard = crate::config::OutboundLeakGuardConfig::default();
        let result = sanitize_gateway_response(input, &tools, &leak_guard);
        assert_eq!(result, "Screenshot captured successfully.");
        assert!(!result.contains("\"name\":\"browser\""));
        assert!(!result.contains("\"result\""));
    }

    #[test]
    fn sanitize_gateway_response_blocks_detected_credentials_when_configured() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        let leak_guard = crate::config::OutboundLeakGuardConfig {
            enabled: true,
            action: crate::config::OutboundLeakGuardAction::Block,
            sensitivity: 0.7,
        };

        let result =
            sanitize_gateway_response("Temporary key: AKIAABCDEFGHIJKLMNOP", &tools, &leak_guard);
        assert!(result.contains("blocked a draft response"));
    }

    #[derive(Default)]
    struct MockMemory;

    #[async_trait]
    impl Memory for MockMemory {
        fn name(&self) -> &str {
            "mock"
        }

        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }

        async fn health_check(&self) -> bool {
            true
        }
    }

    #[derive(Default)]
    struct MockProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok("ok".into())
        }
    }

    #[derive(Default)]
    struct TrackingMemory {
        keys: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Memory for TrackingMemory {
        fn name(&self) -> &str {
            "tracking"
        }

        async fn store(
            &self,
            key: &str,
            _content: &str,
            _category: MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            self.keys.lock().push(key.to_string());
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn get(&self, _key: &str) -> anyhow::Result<Option<MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            let size = self.keys.lock().len();
            Ok(size)
        }

        async fn health_check(&self) -> bool {
            true
        }
    }

    fn test_connect_info() -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 30_300)))
    }

    fn test_public_connect_info() -> ConnectInfo<SocketAddr> {
        ConnectInfo(SocketAddr::from(([203, 0, 113, 10], 30_300)))
    }

    #[tokio::test]
    async fn webhook_idempotency_skips_duplicate_provider_calls() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl.clone();
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let mut headers = HeaderMap::new();
        headers.insert("X-Idempotency-Key", HeaderValue::from_static("abc-123"));

        let body = Ok(Json(WebhookBody {
            message: "hello".into(),
            stream: None,
            session_id: None,
        }));
        let first = handle_webhook(
            State(state.clone()),
            test_connect_info(),
            headers.clone(),
            body,
        )
        .await
        .into_response();
        assert_eq!(first.status(), StatusCode::OK);

        let body = Ok(Json(WebhookBody {
            message: "hello".into(),
            stream: None,
            session_id: None,
        }));
        let second = handle_webhook(State(state), test_connect_info(), headers, body)
            .await
            .into_response();
        assert_eq!(second.status(), StatusCode::OK);

        let payload = second.into_body().collect().await.unwrap().to_bytes();
        let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(parsed["status"], "duplicate");
        assert_eq!(parsed["idempotent"], true);
        assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn webhook_rejects_public_traffic_without_auth_layers() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl;
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_webhook(
            State(state),
            test_public_connect_info(),
            HeaderMap::new(),
            Ok(Json(WebhookBody {
                message: "hello".into(),
                stream: None,
                session_id: None,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_rejects_empty_message() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl.clone();
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_webhook(
            State(state),
            test_connect_info(),
            HeaderMap::new(),
            Ok(Json(WebhookBody {
                message: "   ".into(),
                stream: None,
                session_id: None,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn webhook_stream_response_uses_sse_content_type() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl.clone();
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_webhook(
            State(state),
            test_connect_info(),
            HeaderMap::new(),
            Ok(Json(WebhookBody {
                message: "stream me".into(),
                stream: Some(true),
                session_id: None,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(content_type.starts_with("text/event-stream"));

        let payload = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&payload);
        assert!(text.contains("data: [DONE]"));
        assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn node_control_returns_not_found_when_disabled() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider::default());
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_node_control(
            State(state),
            test_connect_info(),
            HeaderMap::new(),
            Ok(Json(NodeControlRequest {
                method: "node.list".into(),
                node_id: None,
                capability: None,
                arguments: serde_json::Value::Null,
            })),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn node_control_list_returns_stub_nodes_when_enabled() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider::default());
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);

        let mut config = Config::default();
        config.gateway.node_control.enabled = true;
        config.gateway.node_control.allowed_node_ids = vec!["node-1".into(), "node-2".into()];

        let state = AppState {
            config: Arc::new(Mutex::new(config)),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_node_control(
            State(state),
            test_connect_info(),
            HeaderMap::new(),
            Ok(Json(NodeControlRequest {
                method: "node.list".into(),
                node_id: None,
                capability: None,
                arguments: serde_json::Value::Null,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let payload = response.into_body().collect().await.unwrap().to_bytes();
        let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["method"], "node.list");
        assert_eq!(parsed["nodes"].as_array().map(|v| v.len()), Some(2));
    }

    #[tokio::test]
    async fn node_control_rejects_public_requests_without_auth_layers() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider::default());
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);

        let mut config = Config::default();
        config.gateway.node_control.enabled = true;
        config.gateway.node_control.auth_token = None;

        let state = AppState {
            config: Arc::new(Mutex::new(config)),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_node_control(
            State(state),
            test_public_connect_info(),
            HeaderMap::new(),
            Ok(Json(NodeControlRequest {
                method: "node.list".into(),
                node_id: None,
                capability: None,
                arguments: serde_json::Value::Null,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_autosave_stores_distinct_keys_per_request() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl.clone();

        let tracking_impl = Arc::new(TrackingMemory::default());
        let memory: Arc<dyn Memory> = tracking_impl.clone();

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: true,
            webhook_secret_hash: None,
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let headers = HeaderMap::new();

        let body1 = Ok(Json(WebhookBody {
            message: "hello one".into(),
            stream: None,
            session_id: None,
        }));
        let first = handle_webhook(
            State(state.clone()),
            test_connect_info(),
            headers.clone(),
            body1,
        )
        .await
        .into_response();
        assert_eq!(first.status(), StatusCode::OK);

        let body2 = Ok(Json(WebhookBody {
            message: "hello two".into(),
            stream: None,
            session_id: None,
        }));
        let second = handle_webhook(State(state), test_connect_info(), headers, body2)
            .await
            .into_response();
        assert_eq!(second.status(), StatusCode::OK);

        let keys = tracking_impl.keys.lock().clone();
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1]);
        assert!(keys[0].starts_with("webhook_msg_"));
        assert!(keys[1].starts_with("webhook_msg_"));
        assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn webhook_secret_hash_is_deterministic_and_nonempty() {
        let secret_a = generate_test_secret();
        let secret_b = generate_test_secret();
        let one = hash_webhook_secret(&secret_a);
        let two = hash_webhook_secret(&secret_a);
        let other = hash_webhook_secret(&secret_b);

        assert_eq!(one, two);
        assert_ne!(one, other);
        assert_eq!(one.len(), 64);
    }

    #[tokio::test]
    async fn webhook_secret_hash_rejects_missing_header() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl.clone();
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);
        let secret = generate_test_secret();

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: Some(Arc::from(hash_webhook_secret(&secret))),
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let response = handle_webhook(
            State(state),
            test_connect_info(),
            HeaderMap::new(),
            Ok(Json(WebhookBody {
                message: "hello".into(),
                stream: None,
                session_id: None,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn webhook_secret_hash_rejects_invalid_header() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl.clone();
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);
        let valid_secret = generate_test_secret();
        let wrong_secret = generate_test_secret();

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: Some(Arc::from(hash_webhook_secret(&valid_secret))),
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Webhook-Secret",
            HeaderValue::from_str(&wrong_secret).unwrap(),
        );

        let response = handle_webhook(
            State(state),
            test_connect_info(),
            headers,
            Ok(Json(WebhookBody {
                message: "hello".into(),
                stream: None,
                session_id: None,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn webhook_secret_hash_accepts_valid_header() {
        let provider_impl = Arc::new(MockProvider::default());
        let provider: Arc<dyn Provider> = provider_impl.clone();
        let memory: Arc<dyn Memory> = Arc::new(MockMemory);
        let secret = generate_test_secret();

        let state = AppState {
            config: Arc::new(Mutex::new(Config::default())),
            provider,
            model: "test-model".into(),
            temperature: 0.0,
            mem: memory,
            auto_save: false,
            webhook_secret_hash: Some(Arc::from(hash_webhook_secret(&secret))),
            pairing: Arc::new(PairingGuard::new(false, &[], None)),
            trust_forwarded_headers: false,
            rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
            idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
            observer: Arc::new(crate::observability::NoopObserver),
            tools_registry: Arc::new(Vec::new()),
            tools_registry_exec: Arc::new(Vec::new()),
            multimodal: crate::config::MultimodalConfig::default(),
            max_tool_iterations: 10,
            cost_tracker: None,
            event_tx: tokio::sync::broadcast::channel(16).0,
            runtime_trace_path: None,
        };

        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Secret", HeaderValue::from_str(&secret).unwrap());

        let response = handle_webhook(
            State(state),
            test_connect_info(),
            headers,
            Ok(Json(WebhookBody {
                message: "hello".into(),
                stream: None,
                session_id: None,
            })),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 1);
    }

    fn compute_nextcloud_signature_hex(secret: &str, random: &str, body: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let payload = format!("{random}{body}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    // ══════════════════════════════════════════════════════════
    // IdempotencyStore Edge-Case Tests
    // ══════════════════════════════════════════════════════════

    #[test]
    fn idempotency_store_allows_different_keys() {
        let store = IdempotencyStore::new(Duration::from_secs(60), 100);
        assert!(store.record_if_new("key-a"));
        assert!(store.record_if_new("key-b"));
        assert!(store.record_if_new("key-c"));
        assert!(store.record_if_new("key-d"));
    }

    #[test]
    fn idempotency_store_max_keys_clamped_to_one() {
        let store = IdempotencyStore::new(Duration::from_secs(60), 0);
        assert!(store.record_if_new("only-key"));
        assert!(!store.record_if_new("only-key"));
    }

    #[test]
    fn idempotency_store_rapid_duplicate_rejected() {
        let store = IdempotencyStore::new(Duration::from_secs(300), 100);
        assert!(store.record_if_new("rapid"));
        assert!(!store.record_if_new("rapid"));
    }

    #[test]
    fn idempotency_store_accepts_after_ttl_expires() {
        let store = IdempotencyStore::new(Duration::from_millis(1), 100);
        assert!(store.record_if_new("ttl-key"));
        std::thread::sleep(Duration::from_millis(10));
        assert!(store.record_if_new("ttl-key"));
    }

    #[test]
    fn idempotency_store_eviction_preserves_newest() {
        let store = IdempotencyStore::new(Duration::from_secs(300), 1);
        assert!(store.record_if_new("old-key"));
        std::thread::sleep(Duration::from_millis(2));
        assert!(store.record_if_new("new-key"));

        let keys = store.keys.lock();
        assert_eq!(keys.len(), 1);
        assert!(!keys.contains_key("old-key"));
        assert!(keys.contains_key("new-key"));
    }

    #[test]
    fn rate_limiter_allows_after_window_expires() {
        let window = Duration::from_millis(50);
        let limiter = SlidingWindowRateLimiter::new(2, window, 100);
        assert!(limiter.allow("ip-1"));
        assert!(limiter.allow("ip-1"));
        assert!(!limiter.allow("ip-1")); // blocked

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(60));

        // Should be allowed again
        assert!(limiter.allow("ip-1"));
    }

    #[test]
    fn rate_limiter_independent_keys_tracked_separately() {
        let limiter = SlidingWindowRateLimiter::new(2, Duration::from_secs(60), 100);
        assert!(limiter.allow("ip-1"));
        assert!(limiter.allow("ip-1"));
        assert!(!limiter.allow("ip-1")); // ip-1 blocked

        // ip-2 should still work
        assert!(limiter.allow("ip-2"));
        assert!(limiter.allow("ip-2"));
        assert!(!limiter.allow("ip-2")); // ip-2 now blocked
    }

    #[test]
    fn rate_limiter_exact_boundary_at_max_keys() {
        let limiter = SlidingWindowRateLimiter::new(10, Duration::from_secs(60), 3);
        assert!(limiter.allow("ip-1"));
        assert!(limiter.allow("ip-2"));
        assert!(limiter.allow("ip-3"));
        // At capacity now
        assert!(limiter.allow("ip-4")); // should evict ip-1

        let guard = limiter.requests.lock();
        assert_eq!(guard.0.len(), 3);
        assert!(
            !guard.0.contains_key("ip-1"),
            "ip-1 should have been evicted"
        );
        assert!(guard.0.contains_key("ip-2"));
        assert!(guard.0.contains_key("ip-3"));
        assert!(guard.0.contains_key("ip-4"));
    }

    #[test]
    fn gateway_rate_limiter_pair_and_webhook_are_independent() {
        let limiter = GatewayRateLimiter::new(2, 3, 100);

        // Exhaust pair limit
        assert!(limiter.allow_pair("ip-1"));
        assert!(limiter.allow_pair("ip-1"));
        assert!(!limiter.allow_pair("ip-1")); // pair blocked

        // Webhook should still work
        assert!(limiter.allow_webhook("ip-1"));
        assert!(limiter.allow_webhook("ip-1"));
        assert!(limiter.allow_webhook("ip-1"));
        assert!(!limiter.allow_webhook("ip-1")); // webhook now blocked
    }

    #[test]
    fn rate_limiter_single_key_max_allows_one_request() {
        let limiter = SlidingWindowRateLimiter::new(5, Duration::from_secs(60), 1);
        assert!(limiter.allow("ip-1"));
        assert!(limiter.allow("ip-2")); // evicts ip-1

        let guard = limiter.requests.lock();
        assert_eq!(guard.0.len(), 1);
        assert!(guard.0.contains_key("ip-2"));
        assert!(!guard.0.contains_key("ip-1"));
    }

    #[test]
    fn rate_limiter_concurrent_access_safe() {
        use std::sync::Arc;

        let limiter = Arc::new(SlidingWindowRateLimiter::new(
            1000,
            Duration::from_secs(60),
            1000,
        ));
        let mut handles = Vec::new();

        for i in 0..10 {
            let limiter = limiter.clone();
            handles.push(std::thread::spawn(move || {
                for j in 0..100 {
                    limiter.allow(&format!("thread-{i}-req-{j}"));
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Should not panic or deadlock
        let guard = limiter.requests.lock();
        assert!(guard.0.len() <= 1000, "should respect max_keys");
    }

    #[test]
    fn idempotency_store_concurrent_access_safe() {
        use std::sync::Arc;

        let store = Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000));
        let mut handles = Vec::new();

        for i in 0..10 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                for j in 0..100 {
                    store.record_if_new(&format!("thread-{i}-key-{j}"));
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let keys = store.keys.lock();
        assert!(keys.len() <= 1000, "should respect max_keys");
    }

    #[test]
    fn rate_limiter_rapid_burst_then_cooldown() {
        let limiter = SlidingWindowRateLimiter::new(5, Duration::from_millis(50), 100);

        // Burst: use all 5 requests
        for _ in 0..5 {
            assert!(limiter.allow("burst-ip"));
        }
        assert!(!limiter.allow("burst-ip")); // 6th should fail

        // Cooldown
        std::thread::sleep(Duration::from_millis(60));

        // Should be allowed again
        assert!(limiter.allow("burst-ip"));
    }

    #[tokio::test]
    async fn security_headers_are_set_on_responses() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let app =
            Router::new()
                .route("/test", get(|| async { "ok" }))
                .layer(axum::middleware::from_fn(
                    super::security_headers_middleware,
                ));

        let req = Request::builder().uri("/test").body(Body::empty()).unwrap();

        let response = app.oneshot(req).await.unwrap();

        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
        assert_eq!(
            response.headers().get(header::X_FRAME_OPTIONS).unwrap(),
            "DENY"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert_eq!(
            response.headers().get(header::X_XSS_PROTECTION).unwrap(),
            "0"
        );
        assert_eq!(
            response.headers().get(header::REFERRER_POLICY).unwrap(),
            "strict-origin-when-cross-origin"
        );
    }

    #[tokio::test]
    async fn security_headers_are_set_on_error_responses() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let app = Router::new()
            .route(
                "/error",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .layer(axum::middleware::from_fn(
                super::security_headers_middleware,
            ));

        let req = Request::builder()
            .uri("/error")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
        assert_eq!(
            response.headers().get(header::X_FRAME_OPTIONS).unwrap(),
            "DENY"
        );
    }
}
