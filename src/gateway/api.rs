//! REST API handlers for the web dashboard.
//!
//! All `/api/*` routes require bearer token authentication (PairingGuard).

use super::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json},
};
use serde::Deserialize;

const MASKED_SECRET: &str = "***MASKED***";

// ── Bearer token auth extractor ─────────────────────────────────

/// Extract and validate bearer token from Authorization header.
fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
}

/// Verify bearer token against PairingGuard. Returns error response if unauthorized.
fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if !state.pairing.require_pairing() {
        return Ok(());
    }

    let token = extract_bearer_token(headers).unwrap_or("");
    if state.pairing.is_authenticated(token) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Unauthorized — pair first via POST /pair, then send Authorization: Bearer <token>"
            })),
        ))
    }
}

// ── Query parameters ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MemoryQuery {
    pub query: Option<String>,
    pub category: Option<String>,
}

#[derive(Deserialize)]
pub struct MemoryStoreBody {
    pub key: String,
    pub content: String,
    pub category: Option<String>,
}

#[derive(Deserialize)]
pub struct CronAddBody {
    pub name: Option<String>,
    pub schedule: String,
    pub command: String,
}

// ── Handlers ────────────────────────────────────────────────────

/// GET /api/status — system status overview
pub async fn handle_api_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let health = crate::health::snapshot();

    let mut channels = serde_json::Map::new();

    for (channel, present) in config.channels_config.channels() {
        channels.insert(channel.name().to_string(), serde_json::Value::Bool(present));
    }

    let body = serde_json::json!({
        "version": env!("ZEROCLAW_BUILD_VERSION"),
        "provider": format_provider_display(&config.default_provider),
        "model": state.model,
        "temperature": state.temperature,
        "uptime_seconds": health.uptime_seconds,
        "gateway_port": config.gateway.port,
        "locale": "en",
        "memory_backend": state.mem.name(),
        "paired": state.pairing.is_paired(),
        "channels": channels,
        "health": health,
    });

    Json(body).into_response()
}

/// Format provider name for display — strip URL suffixes from custom providers.
fn format_provider_display(raw: &Option<String>) -> Option<String> {
    raw.as_ref().map(|s| {
        if let Some(prefix) = s.split(':').next() {
            if s.contains("://") {
                // e.g. "custom:http://..." → "Custom"
                prefix
                    .split('-')
                    .map(|w| {
                        let mut c = w.chars();
                        match c.next() {
                            Some(f) => f.to_uppercase().to_string() + c.as_str(),
                            None => String::new(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                s.clone()
            }
        } else {
            s.clone()
        }
    })
}

/// GET /api/config — current config (api_key masked)
pub async fn handle_api_config_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();

    // Serialize to TOML after masking sensitive fields.
    let masked_config = mask_sensitive_fields(&config);
    let toml_str = match toml::to_string_pretty(&masked_config) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to serialize config: {e}")})),
            )
                .into_response();
        }
    };

    Json(serde_json::json!({
        "format": "toml",
        "content": toml_str,
    }))
    .into_response()
}

/// PUT /api/config — update config from TOML body
pub async fn handle_api_config_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    // Parse the incoming TOML and normalize known dashboard-masked edge cases.
    let mut incoming_toml: toml::Value = match toml::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid TOML: {e}")})),
            )
                .into_response();
        }
    };
    normalize_dashboard_config_toml(&mut incoming_toml);
    let incoming: crate::config::Config = match incoming_toml.try_into() {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid TOML: {e}")})),
            )
                .into_response();
        }
    };

    let current_config = state.config.lock().clone();
    let new_config = hydrate_config_for_save(incoming, &current_config);

    if let Err(e) = new_config.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid config: {e}")})),
        )
            .into_response();
    }

    // Save to disk
    if let Err(e) = new_config.save().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to save config: {e}")})),
        )
            .into_response();
    }

    // Update in-memory config
    *state.config.lock() = new_config;

    Json(serde_json::json!({"status": "ok"})).into_response()
}

/// GET /api/tools — list registered tool specs
pub async fn handle_api_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let tools: Vec<serde_json::Value> = state
        .tools_registry
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.parameters,
            })
        })
        .collect();

    Json(serde_json::json!({"tools": tools})).into_response()
}

/// GET /api/skills — list installed skills
pub async fn handle_api_skills(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config_guard = state.config.lock();
    let skills = crate::skills::load_skills_with_config(&config_guard.workspace_dir, &config_guard);

    let skills_json: Vec<serde_json::Value> = skills
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "version": s.version,
                "author": s.author,
                "tags": s.tags,
                "tools": s.tools.iter().map(|t| serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "kind": t.kind,
                })).collect::<Vec<_>>(),
                "location": s.location.map(|p| p.display().to_string()),
            })
        })
        .collect();

    Json(serde_json::json!({"skills": skills_json})).into_response()
}

/// GET /api/cron — list cron jobs
pub async fn handle_api_cron_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    match crate::cron::list_jobs(&config) {
        Ok(jobs) => {
            let jobs_json: Vec<serde_json::Value> = jobs
                .iter()
                .map(|job| {
                    serde_json::json!({
                        "id": job.id,
                        "name": job.name,
                        "command": job.command,
                        "next_run": job.next_run.to_rfc3339(),
                        "last_run": job.last_run.map(|t| t.to_rfc3339()),
                        "last_status": job.last_status,
                        "enabled": job.enabled,
                    })
                })
                .collect();
            Json(serde_json::json!({"jobs": jobs_json})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to list cron jobs: {e}")})),
        )
            .into_response(),
    }
}

/// POST /api/cron — add a new cron job
pub async fn handle_api_cron_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CronAddBody>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let schedule = crate::cron::Schedule::Cron {
        expr: body.schedule,
        tz: None,
    };

    match crate::cron::add_shell_job_with_approval(
        &config,
        body.name,
        schedule,
        &body.command,
        false,
    ) {
        Ok(job) => Json(serde_json::json!({
            "status": "ok",
            "job": {
                "id": job.id,
                "name": job.name,
                "command": job.command,
                "enabled": job.enabled,
            }
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to add cron job: {e}")})),
        )
            .into_response(),
    }
}

/// DELETE /api/cron/:id — remove a cron job
pub async fn handle_api_cron_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    match crate::cron::remove_job(&config, &id) {
        Ok(()) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to remove cron job: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/integrations — list all integrations with status
pub async fn handle_api_integrations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let entries = crate::integrations::registry::all_integrations();

    let integrations: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let status = (entry.status_fn)(&config);
            serde_json::json!({
                "name": entry.name,
                "description": entry.description,
                "category": entry.category,
                "status": status,
            })
        })
        .collect();

    Json(serde_json::json!({"integrations": integrations})).into_response()
}

/// GET /api/integrations/settings — detailed settings for each integration
pub async fn handle_api_integrations_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let entries = crate::integrations::registry::all_integrations();

    let active_default_provider_id = config
        .default_provider
        .as_ref()
        .and_then(|p| integration_id_from_provider(p));

    let integrations: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let status = (entry.status_fn)(&config);
            let (configured, fields) = integration_settings_fields(&config, entry.name);
            let activates_default_provider = is_ai_provider(entry.name);

            serde_json::json!({
                "id": integration_name_to_id(entry.name),
                "name": entry.name,
                "description": entry.description,
                "category": entry.category,
                "status": status,
                "configured": configured,
                "activates_default_provider": activates_default_provider,
                "fields": fields,
            })
        })
        .collect();

    Json(serde_json::json!({
        "revision": "v1",
        "active_default_provider_integration_id": active_default_provider_id,
        "integrations": integrations,
    }))
    .into_response()
}

/// PUT /api/integrations/:id/credentials — update integration credentials
pub async fn handle_api_integrations_credentials_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let fields = body
        .get("fields")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut config = state.config.lock().clone();
    let Some(provider_key) = provider_key_from_integration_id(&id) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Integration '{}' does not support credential updates via this endpoint",
                    id
                )
            })),
        )
            .into_response();
    };

    // Apply credential updates based on integration
    match provider_key {
        "openrouter" | "anthropic" | "openai" | "google" | "deepseek" | "xai" | "mistral"
        | "perplexity" | "bedrock" | "groq" | "together" | "cohere" | "fireworks" | "venice"
        | "moonshot" | "stepfun" | "synthetic" | "opencode" | "zai" | "glm" | "minimax"
        | "qwen" | "qianfan" | "doubao" | "volcengine" | "ark" | "siliconflow" => {
            if let Some(api_key) = fields.get("api_key").and_then(|v| v.as_str()) {
                if !api_key.is_empty() && api_key != MASKED_SECRET {
                    config.api_key = Some(api_key.to_string());
                }
            }
            if let Some(default_model) = fields.get("default_model").and_then(|v| v.as_str()) {
                if !default_model.is_empty() {
                    config.default_model = Some(default_model.to_string());
                }
            }
            config.default_provider = Some(provider_key.to_string());
        }
        "ollama" => {
            if let Some(default_model) = fields.get("default_model").and_then(|v| v.as_str()) {
                if !default_model.is_empty() {
                    config.default_model = Some(default_model.to_string());
                }
            }
            config.default_provider = Some("ollama".to_string());
        }
        "notion" => {
            if let Some(api_key) = fields.get("api_key").and_then(|v| v.as_str()) {
                if !api_key.is_empty() && api_key != MASKED_SECRET {
                    config.notion.api_key = api_key.to_string();
                }
            }
            if let Some(database_id) = fields.get("database_id").and_then(|v| v.as_str()) {
                if !database_id.is_empty() {
                    config.notion.database_id = database_id.to_string();
                }
            }
            if !config.notion.api_key.is_empty() && !config.notion.database_id.is_empty() {
                config.notion.enabled = true;
            }
        }
        "jira" => {
            if let Some(base_url) = fields.get("base_url").and_then(|v| v.as_str()) {
                if !base_url.is_empty() && base_url != MASKED_SECRET {
                    config.atlassian.base_url = base_url.to_string();
                }
            }
            if let Some(email) = fields.get("email").and_then(|v| v.as_str()) {
                if !email.is_empty() && email != MASKED_SECRET {
                    config.atlassian.email = email.to_string();
                }
            }
            if let Some(api_token) = fields.get("api_token").and_then(|v| v.as_str()) {
                if !api_token.is_empty() && api_token != MASKED_SECRET {
                    config.atlassian.api_token = api_token.to_string();
                }
            }
            if !config.atlassian.base_url.is_empty()
                && !config.atlassian.email.is_empty()
                && !config.atlassian.api_token.is_empty()
            {
                config.atlassian.jira_enabled = true;
            }
        }
        _ => {
            // Channel integrations - not implemented for credentials update via this endpoint
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Integration '{}' does not support credential updates via this endpoint", id)
                })),
            )
                .into_response();
        }
    }

    // Save config
    if let Err(e) = config.save().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to save config: {e}")})),
        )
            .into_response();
    }

    // Update in-memory config
    *state.config.lock() = config;

    Json(serde_json::json!({
        "status": "ok",
        "revision": "v1",
    }))
    .into_response()
}

fn integration_name_to_id(name: &str) -> String {
    name.to_lowercase()
        .replace(' ', "-")
        .replace(['/', '.'], "-")
}

fn provider_key_from_integration_id(id: &str) -> Option<&'static str> {
    match id {
        "openrouter" => Some("openrouter"),
        "anthropic" => Some("anthropic"),
        "openai" => Some("openai"),
        "google" => Some("google"),
        "deepseek" => Some("deepseek"),
        "xai" => Some("xai"),
        "mistral" => Some("mistral"),
        "perplexity" => Some("perplexity"),
        "amazon-bedrock" => Some("bedrock"),
        "groq" => Some("groq"),
        "together-ai" => Some("together"),
        "cohere" => Some("cohere"),
        "fireworks-ai" => Some("fireworks"),
        "venice" => Some("venice"),
        "moonshot" => Some("moonshot"),
        "stepfun" => Some("stepfun"),
        "synthetic" => Some("synthetic"),
        "opencode-zen" => Some("opencode"),
        "z-ai" => Some("zai"),
        "glm" => Some("glm"),
        "minimax" => Some("minimax"),
        "qwen" => Some("qwen"),
        "qianfan" => Some("qianfan"),
        "volcengine-ark" => Some("ark"),
        "siliconflow" => Some("siliconflow"),
        "ollama" => Some("ollama"),
        "notion" => Some("notion"),
        "jira" => Some("jira"),
        _ => None,
    }
}

fn is_ai_provider(name: &str) -> bool {
    matches!(
        name,
        "OpenRouter"
            | "Anthropic"
            | "OpenAI"
            | "Google"
            | "DeepSeek"
            | "xAI"
            | "Mistral"
            | "Perplexity"
            | "Vercel AI"
            | "Amazon Bedrock"
            | "Groq"
            | "Together AI"
            | "Cohere"
            | "Fireworks AI"
            | "Venice"
            | "Moonshot"
            | "StepFun"
            | "Synthetic"
            | "OpenCode Zen"
            | "Z.AI"
            | "GLM"
            | "MiniMax"
            | "Qwen"
            | "Qianfan"
            | "Volcengine ARK"
            | "SiliconFlow"
            | "Ollama"
    )
}

fn integration_id_from_provider(provider: &str) -> Option<String> {
    let name = match provider {
        "openrouter" => "OpenRouter",
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "google" | "vertex" => "Google",
        "deepseek" => "DeepSeek",
        "xai" | "x-ai" => "xAI",
        "mistral" => "Mistral",
        "perplexity" => "Perplexity",
        "bedrock" => "Amazon Bedrock",
        "groq" => "Groq",
        "together" => "Together AI",
        "cohere" => "Cohere",
        "fireworks" => "Fireworks AI",
        "venice" => "Venice",
        "moonshot" | "moonshot-cn" | "moonshot-intl" => "Moonshot",
        "stepfun" | "step-ai" => "StepFun",
        "synthetic" => "Synthetic",
        "opencode" => "OpenCode Zen",
        "zai" | "zai-cn" | "zai-intl" => "Z.AI",
        "glm" | "glm-cn" | "glm-intl" => "GLM",
        "minimax" | "minimax-cn" | "minimax-intl" => "MiniMax",
        "qwen" | "qwen-cn" | "qwen-intl" => "Qwen",
        "qianfan" | "baidu" => "Qianfan",
        "doubao" | "volcengine" | "ark" => "Volcengine ARK",
        "siliconflow" | "silicon-cloud" => "SiliconFlow",
        "ollama" => "Ollama",
        _ => return None,
    };
    Some(integration_name_to_id(name))
}

#[allow(clippy::too_many_lines)]
fn integration_settings_fields(
    config: &crate::config::Config,
    name: &str,
) -> (bool, Vec<serde_json::Value>) {
    match name {
        "OpenRouter" => {
            let has_key = config.api_key.is_some();
            let fields = vec![
                serde_json::json!({
                    "key": "api_key",
                    "label": "API Key",
                    "required": true,
                    "has_value": has_key,
                    "input_type": "secret",
                    "options": [],
                    "masked_value": if has_key { Some(MASKED_SECRET) } else { None },
                }),
                serde_json::json!({
                    "key": "default_model",
                    "label": "Default Model",
                    "required": false,
                    "has_value": config.default_model.is_some(),
                    "input_type": "select",
                    "options": [
                        "anthropic/claude-sonnet-4-6",
                        "openai/gpt-5.2",
                        "google/gemini-3.1-pro",
                        "deepseek/deepseek-reasoner",
                        "x-ai/grok-4",
                    ],
                    "current_value": config.default_model.as_deref().unwrap_or(""),
                }),
            ];
            (has_key, fields)
        }
        "Anthropic" => {
            let has_key = config.api_key.is_some();
            let fields = vec![
                serde_json::json!({
                    "key": "api_key",
                    "label": "API Key",
                    "required": true,
                    "has_value": has_key,
                    "input_type": "secret",
                    "options": [],
                    "masked_value": if has_key { Some(MASKED_SECRET) } else { None },
                }),
                serde_json::json!({
                    "key": "default_model",
                    "label": "Default Model",
                    "required": false,
                    "has_value": config.default_model.is_some(),
                    "input_type": "select",
                    "options": ["claude-sonnet-4-6", "claude-opus-4-6"],
                    "current_value": config.default_model.as_deref().unwrap_or(""),
                }),
            ];
            (has_key, fields)
        }
        "OpenAI" => {
            let has_key = config.api_key.is_some();
            let fields = vec![
                serde_json::json!({
                    "key": "api_key",
                    "label": "API Key",
                    "required": true,
                    "has_value": has_key,
                    "input_type": "secret",
                    "options": [],
                    "masked_value": if has_key { Some(MASKED_SECRET) } else { None },
                }),
                serde_json::json!({
                    "key": "default_model",
                    "label": "Default Model",
                    "required": false,
                    "has_value": config.default_model.is_some(),
                    "input_type": "select",
                    "options": ["gpt-5.2", "gpt-5.2-codex", "gpt-4o"],
                    "current_value": config.default_model.as_deref().unwrap_or(""),
                }),
            ];
            (has_key, fields)
        }
        "Notion" => {
            let has_key = !config.notion.api_key.is_empty();
            let has_db = !config.notion.database_id.is_empty();
            let configured = has_key && has_db;
            let fields = vec![
                serde_json::json!({
                    "key": "api_key",
                    "label": "API Key",
                    "required": true,
                    "has_value": has_key,
                    "input_type": "secret",
                    "options": [],
                    "masked_value": if has_key { Some(MASKED_SECRET) } else { None::<&str> },
                }),
                serde_json::json!({
                    "key": "database_id",
                    "label": "Database ID",
                    "required": true,
                    "has_value": has_db,
                    "input_type": "text",
                    "options": [],
                    "current_value": if has_db { &config.notion.database_id } else { "" },
                }),
            ];
            (configured, fields)
        }
        "Jira" => {
            let has_token = !config.atlassian.api_token.is_empty();
            let has_url = !config.atlassian.base_url.is_empty();
            let has_email = !config.atlassian.email.is_empty();
            let configured = has_token && has_url && has_email;
            let fields = vec![
                serde_json::json!({
                    "key": "base_url",
                    "label": "Base URL",
                    "required": true,
                    "has_value": has_url,
                    "input_type": "secret",
                    "options": [],
                    "masked_value": if has_url { Some(MASKED_SECRET) } else { None::<&str> },
                }),
                serde_json::json!({
                    "key": "email",
                    "label": "Email",
                    "required": true,
                    "has_value": has_email,
                    "input_type": "secret",
                    "options": [],
                    "masked_value": if has_email { Some(MASKED_SECRET) } else { None::<&str> },
                }),
                serde_json::json!({
                    "key": "api_token",
                    "label": "API Token",
                    "required": true,
                    "has_value": has_token,
                    "input_type": "secret",
                    "options": [],
                    "masked_value": if has_token { Some(MASKED_SECRET) } else { None::<&str> },
                }),
            ];
            (configured, fields)
        }
        _ => {
            // Default: no configurable fields
            (false, vec![])
        }
    }
}

/// POST /api/doctor — run diagnostics
pub async fn handle_api_doctor(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let results = crate::doctor::diagnose(&config);

    let ok_count = results
        .iter()
        .filter(|r| r.severity == crate::doctor::Severity::Ok)
        .count();
    let warn_count = results
        .iter()
        .filter(|r| r.severity == crate::doctor::Severity::Warn)
        .count();
    let error_count = results
        .iter()
        .filter(|r| r.severity == crate::doctor::Severity::Error)
        .count();

    Json(serde_json::json!({
        "results": results,
        "summary": {
            "ok": ok_count,
            "warnings": warn_count,
            "errors": error_count,
        }
    }))
    .into_response()
}

/// GET /api/memory — list or search memory entries
pub async fn handle_api_memory_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<MemoryQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    if let Some(ref query) = params.query {
        // Search mode
        match state.mem.recall(query, 50, None).await {
            Ok(entries) => Json(serde_json::json!({"entries": entries})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Memory recall failed: {e}")})),
            )
                .into_response(),
        }
    } else {
        // List mode
        let category = params.category.as_deref().map(|cat| match cat {
            "core" => crate::memory::MemoryCategory::Core,
            "daily" => crate::memory::MemoryCategory::Daily,
            "conversation" => crate::memory::MemoryCategory::Conversation,
            other => crate::memory::MemoryCategory::Custom(other.to_string()),
        });

        match state.mem.list(category.as_ref(), None).await {
            Ok(entries) => Json(serde_json::json!({"entries": entries})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Memory list failed: {e}")})),
            )
                .into_response(),
        }
    }
}

/// POST /api/memory — store a memory entry
pub async fn handle_api_memory_store(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MemoryStoreBody>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let category = body
        .category
        .as_deref()
        .map(|cat| match cat {
            "core" => crate::memory::MemoryCategory::Core,
            "daily" => crate::memory::MemoryCategory::Daily,
            "conversation" => crate::memory::MemoryCategory::Conversation,
            other => crate::memory::MemoryCategory::Custom(other.to_string()),
        })
        .unwrap_or(crate::memory::MemoryCategory::Core);

    match state
        .mem
        .store(&body.key, &body.content, category, None)
        .await
    {
        Ok(()) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Memory store failed: {e}")})),
        )
            .into_response(),
    }
}

/// DELETE /api/memory/:key — delete a memory entry
pub async fn handle_api_memory_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    match state.mem.forget(&key).await {
        Ok(deleted) => {
            Json(serde_json::json!({"status": "ok", "deleted": deleted})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Memory forget failed: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/cost — cost summary
pub async fn handle_api_cost(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    if let Some(ref tracker) = state.cost_tracker {
        match tracker.get_summary() {
            Ok(summary) => Json(serde_json::json!({"cost": summary})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Cost summary failed: {e}")})),
            )
                .into_response(),
        }
    } else {
        Json(serde_json::json!({
            "cost": {
                "hourly_cost_usd": 0.0,
                "daily_cost_usd": 0.0,
                "monthly_cost_usd": 0.0,
                "total_tokens": 0,
                "request_count": 0,
                "by_model": {},
            }
        }))
        .into_response()
    }
}

/// GET /api/cli-tools — discovered CLI tools
pub async fn handle_api_cli_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let tools = tokio::task::spawn_blocking(|| {
        crate::util::discover_cli_tools(&[], &[])
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!("CLI tool discovery task panicked: {}", e);
        Vec::new()
    });

    Json(serde_json::json!({"cli_tools": tools})).into_response()
}

/// GET /api/health — component health snapshot
pub async fn handle_api_health(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let snapshot = crate::health::snapshot();
    Json(serde_json::json!({"health": snapshot})).into_response()
}

/// GET /api/pairing/devices — list paired devices
pub async fn handle_api_pairing_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let devices = state.pairing.paired_devices();
    Json(serde_json::json!({ "devices": devices })).into_response()
}

/// DELETE /api/pairing/devices/:id — revoke paired device
pub async fn handle_api_pairing_device_revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    if !state.pairing.revoke_device(&id) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Paired device not found"})),
        )
            .into_response();
    }

    if let Err(e) = super::persist_pairing_tokens(state.config.clone(), &state.pairing).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to persist pairing state: {e}")})),
        )
            .into_response();
    }

    Json(serde_json::json!({"status": "ok", "revoked": true, "id": id})).into_response()
}

// ── Helpers ─────────────────────────────────────────────────────

fn normalize_dashboard_config_toml(root: &mut toml::Value) {
    // Dashboard editors may round-trip masked reliability api_keys as a single
    // string. Accept that shape by normalizing it back to a string array.
    let Some(root_table) = root.as_table_mut() else {
        return;
    };
    let Some(reliability) = root_table
        .get_mut("reliability")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    let Some(api_keys) = reliability.get_mut("api_keys") else {
        return;
    };
    if let Some(single) = api_keys.as_str() {
        *api_keys = toml::Value::Array(vec![toml::Value::String(single.to_string())]);
    }
}

fn is_masked_secret(value: &str) -> bool {
    value == MASKED_SECRET
}

fn mask_optional_secret(value: &mut Option<String>) {
    if value.is_some() {
        *value = Some(MASKED_SECRET.to_string());
    }
}

fn mask_required_secret(value: &mut String) {
    if !value.is_empty() {
        *value = MASKED_SECRET.to_string();
    }
}

fn mask_vec_secrets(values: &mut [String]) {
    for value in values.iter_mut() {
        if !value.is_empty() {
            *value = MASKED_SECRET.to_string();
        }
    }
}

#[allow(clippy::ref_option)]
fn restore_optional_secret(value: &mut Option<String>, current: &Option<String>) {
    if value.as_deref().is_some_and(is_masked_secret) {
        *value = current.clone();
    }
}

fn restore_required_secret(value: &mut String, current: &str) {
    if is_masked_secret(value) {
        *value = current.to_string();
    }
}

fn restore_vec_secrets(values: &mut [String], current: &[String]) {
    for (idx, value) in values.iter_mut().enumerate() {
        if is_masked_secret(value) {
            if let Some(existing) = current.get(idx) {
                *value = existing.clone();
            }
        }
    }
}

fn mask_map_secrets(values: &mut std::collections::HashMap<String, String>) {
    for value in values.values_mut() {
        if !value.is_empty() {
            *value = MASKED_SECRET.to_string();
        }
    }
}

fn restore_map_secrets(
    values: &mut std::collections::HashMap<String, String>,
    current: &std::collections::HashMap<String, String>,
) {
    for (key, value) in values.iter_mut() {
        if is_masked_secret(value) {
            if let Some(existing) = current.get(key) {
                *value = existing.clone();
            }
        }
    }
}

fn mask_sensitive_fields(config: &crate::config::Config) -> crate::config::Config {
    let mut masked = config.clone();

    mask_optional_secret(&mut masked.api_key);
    mask_vec_secrets(&mut masked.reliability.api_keys);
    mask_map_secrets(&mut masked.reliability.fallback_api_keys);
    mask_optional_secret(&mut masked.composio.api_key);
    mask_optional_secret(&mut masked.proxy.http_proxy);
    mask_optional_secret(&mut masked.proxy.https_proxy);
    mask_optional_secret(&mut masked.proxy.all_proxy);
    mask_optional_secret(&mut masked.transcription.api_key);
    mask_optional_secret(&mut masked.browser.computer_use.api_key);
    mask_optional_secret(&mut masked.web_search.brave_api_key);
    mask_optional_secret(&mut masked.storage.provider.config.db_url);
    if let Some(cloudflare) = masked.tunnel.cloudflare.as_mut() {
        mask_required_secret(&mut cloudflare.token);
    }
    if let Some(ngrok) = masked.tunnel.ngrok.as_mut() {
        mask_required_secret(&mut ngrok.auth_token);
    }

    for agent in masked.agents.values_mut() {
        mask_optional_secret(&mut agent.api_key);
    }
    for provider in masked.model_providers.values_mut() {
        mask_optional_secret(&mut provider.api_key);
    }
    mask_vec_secrets(&mut masked.gateway.paired_tokens);
    mask_required_secret(&mut masked.notion.api_key);
    mask_required_secret(&mut masked.atlassian.api_token);
    mask_required_secret(&mut masked.atlassian.email);
    mask_required_secret(&mut masked.atlassian.base_url);

    if let Some(discord) = masked.channels_config.discord.as_mut() {
        mask_required_secret(&mut discord.bot_token);
    }
    if let Some(slack) = masked.channels_config.slack.as_mut() {
        mask_required_secret(&mut slack.bot_token);
        mask_optional_secret(&mut slack.app_token);
    }
    if let Some(webhook) = masked.channels_config.webhook.as_mut() {
        mask_optional_secret(&mut webhook.secret);
    }
    if let Some(github) = masked.channels_config.github.as_mut() {
        mask_required_secret(&mut github.access_token);
        mask_optional_secret(&mut github.webhook_secret);
    }
    if let Some(email) = masked.channels_config.email.as_mut() {
        mask_required_secret(&mut email.password);
    }
    if let Some(irc) = masked.channels_config.irc.as_mut() {
        mask_optional_secret(&mut irc.server_password);
        mask_optional_secret(&mut irc.nickserv_password);
        mask_optional_secret(&mut irc.sasl_password);
    }
    masked
}

fn restore_masked_sensitive_fields(
    incoming: &mut crate::config::Config,
    current: &crate::config::Config,
) {
    restore_optional_secret(&mut incoming.api_key, &current.api_key);
    restore_vec_secrets(
        &mut incoming.reliability.api_keys,
        &current.reliability.api_keys,
    );
    restore_map_secrets(
        &mut incoming.reliability.fallback_api_keys,
        &current.reliability.fallback_api_keys,
    );
    restore_optional_secret(&mut incoming.composio.api_key, &current.composio.api_key);
    restore_optional_secret(&mut incoming.proxy.http_proxy, &current.proxy.http_proxy);
    restore_optional_secret(&mut incoming.proxy.https_proxy, &current.proxy.https_proxy);
    restore_optional_secret(&mut incoming.proxy.all_proxy, &current.proxy.all_proxy);
    restore_optional_secret(
        &mut incoming.transcription.api_key,
        &current.transcription.api_key,
    );
    restore_optional_secret(
        &mut incoming.browser.computer_use.api_key,
        &current.browser.computer_use.api_key,
    );

    restore_optional_secret(
        &mut incoming.web_search.brave_api_key,
        &current.web_search.brave_api_key,
    );
    restore_optional_secret(
        &mut incoming.storage.provider.config.db_url,
        &current.storage.provider.config.db_url,
    );
    if let (Some(incoming_tunnel), Some(current_tunnel)) = (
        incoming.tunnel.cloudflare.as_mut(),
        current.tunnel.cloudflare.as_ref(),
    ) {
        restore_required_secret(&mut incoming_tunnel.token, &current_tunnel.token);
    }
    if let (Some(incoming_tunnel), Some(current_tunnel)) = (
        incoming.tunnel.ngrok.as_mut(),
        current.tunnel.ngrok.as_ref(),
    ) {
        restore_required_secret(&mut incoming_tunnel.auth_token, &current_tunnel.auth_token);
    }

    for (name, agent) in &mut incoming.agents {
        if let Some(current_agent) = current.agents.get(name) {
            restore_optional_secret(&mut agent.api_key, &current_agent.api_key);
        }
    }
    for (name, provider) in &mut incoming.model_providers {
        if let Some(current_provider) = current.model_providers.get(name) {
            restore_optional_secret(&mut provider.api_key, &current_provider.api_key);
        }
    }
    restore_vec_secrets(
        &mut incoming.gateway.paired_tokens,
        &current.gateway.paired_tokens,
    );
    restore_required_secret(&mut incoming.notion.api_key, &current.notion.api_key);
    restore_required_secret(&mut incoming.atlassian.api_token, &current.atlassian.api_token);
    restore_required_secret(&mut incoming.atlassian.email, &current.atlassian.email);
    restore_required_secret(&mut incoming.atlassian.base_url, &current.atlassian.base_url);

    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.discord.as_mut(),
        current.channels_config.discord.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.bot_token, &current_ch.bot_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.slack.as_mut(),
        current.channels_config.slack.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.bot_token, &current_ch.bot_token);
        restore_optional_secret(&mut incoming_ch.app_token, &current_ch.app_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.webhook.as_mut(),
        current.channels_config.webhook.as_ref(),
    ) {
        restore_optional_secret(&mut incoming_ch.secret, &current_ch.secret);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.github.as_mut(),
        current.channels_config.github.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.access_token, &current_ch.access_token);
        restore_optional_secret(&mut incoming_ch.webhook_secret, &current_ch.webhook_secret);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.email.as_mut(),
        current.channels_config.email.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.password, &current_ch.password);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.irc.as_mut(),
        current.channels_config.irc.as_ref(),
    ) {
        restore_optional_secret(
            &mut incoming_ch.server_password,
            &current_ch.server_password,
        );
        restore_optional_secret(
            &mut incoming_ch.nickserv_password,
            &current_ch.nickserv_password,
        );
        restore_optional_secret(&mut incoming_ch.sasl_password, &current_ch.sasl_password);
    }
}

fn hydrate_config_for_save(
    mut incoming: crate::config::Config,
    current: &crate::config::Config,
) -> crate::config::Config {
    restore_masked_sensitive_fields(&mut incoming, current);
    // These are runtime-computed fields skipped from TOML serialization.
    incoming.config_path = current.config_path.clone();
    incoming.workspace_dir = current.workspace_dir.clone();
    incoming
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking_keeps_toml_valid_and_preserves_api_keys_type() {
        let mut cfg = crate::config::Config::default();
        cfg.api_key = Some("sk-live-123".to_string());
        cfg.reliability.api_keys = vec!["rk-1".to_string(), "rk-2".to_string()];

        let masked = mask_sensitive_fields(&cfg);
        let toml = toml::to_string_pretty(&masked).expect("masked config should serialize");
        let parsed: crate::config::Config =
            toml::from_str(&toml).expect("masked config should remain valid TOML for Config");

        assert_eq!(parsed.api_key.as_deref(), Some(MASKED_SECRET));
        assert_eq!(
            parsed.reliability.api_keys,
            vec![MASKED_SECRET.to_string(), MASKED_SECRET.to_string()]
        );
    }

    #[test]
    fn hydrate_config_for_save_restores_masked_secrets_and_paths() {
        let mut current = crate::config::Config::default();
        current.config_path = std::path::PathBuf::from("/tmp/current/config.toml");
        current.workspace_dir = std::path::PathBuf::from("/tmp/current/workspace");
        current.api_key = Some("real-key".to_string());
        current.transcription.api_key = Some("transcription-real-key".to_string());
        current.reliability.api_keys = vec!["r1".to_string(), "r2".to_string()];

        let mut incoming = mask_sensitive_fields(&current);
        incoming.default_model = Some("gpt-4.1-mini".to_string());
        // Simulate UI changing only one key and keeping the first masked.
        incoming.reliability.api_keys = vec![MASKED_SECRET.to_string(), "r2-new".to_string()];

        let hydrated = hydrate_config_for_save(incoming, &current);

        assert_eq!(hydrated.config_path, current.config_path);
        assert_eq!(hydrated.workspace_dir, current.workspace_dir);
        assert_eq!(hydrated.api_key, current.api_key);
        assert_eq!(
            hydrated.transcription.api_key,
            current.transcription.api_key
        );
        assert_eq!(hydrated.default_model.as_deref(), Some("gpt-4.1-mini"));
        assert_eq!(
            hydrated.reliability.api_keys,
            vec!["r1".to_string(), "r2-new".to_string()]
        );
    }

    #[test]
    fn normalize_dashboard_config_toml_promotes_single_api_key_string_to_array() {
        let mut cfg = crate::config::Config::default();
        cfg.reliability.api_keys = vec!["rk-live".to_string()];
        let raw_toml = toml::to_string_pretty(&cfg).expect("config should serialize");
        let mut raw =
            toml::from_str::<toml::Value>(&raw_toml).expect("serialized config should parse");
        raw.as_table_mut()
            .and_then(|root| root.get_mut("reliability"))
            .and_then(toml::Value::as_table_mut)
            .and_then(|reliability| reliability.get_mut("api_keys"))
            .map(|api_keys| *api_keys = toml::Value::String(MASKED_SECRET.to_string()))
            .expect("reliability.api_keys should exist");

        normalize_dashboard_config_toml(&mut raw);

        let parsed: crate::config::Config = raw
            .try_into()
            .expect("normalized toml should parse as Config");
        assert_eq!(parsed.reliability.api_keys, vec![MASKED_SECRET.to_string()]);
    }

    #[test]
    fn provider_key_from_integration_id_maps_dashboard_ids() {
        assert_eq!(provider_key_from_integration_id("openai"), Some("openai"));
        assert_eq!(
            provider_key_from_integration_id("amazon-bedrock"),
            Some("bedrock")
        );
        assert_eq!(
            provider_key_from_integration_id("together-ai"),
            Some("together")
        );
        assert_eq!(
            provider_key_from_integration_id("opencode-zen"),
            Some("opencode")
        );
        assert_eq!(
            provider_key_from_integration_id("volcengine-ark"),
            Some("ark")
        );
        assert_eq!(provider_key_from_integration_id("slack"), None);
    }

    #[test]
    fn integration_provider_mapping_roundtrips_for_supported_providers() {
        let cases = vec![
            ("openrouter", "openrouter"),
            ("anthropic", "anthropic"),
            ("openai", "openai"),
            ("google", "google"),
            ("deepseek", "deepseek"),
            ("xai", "xai"),
            ("mistral", "mistral"),
            ("perplexity", "perplexity"),
            ("bedrock", "bedrock"),
            ("groq", "groq"),
            ("together", "together"),
            ("cohere", "cohere"),
            ("fireworks", "fireworks"),
            ("venice", "venice"),
            ("moonshot", "moonshot"),
            ("stepfun", "stepfun"),
            ("synthetic", "synthetic"),
            ("opencode", "opencode"),
            ("zai", "zai"),
            ("glm", "glm"),
            ("minimax", "minimax"),
            ("qwen", "qwen"),
            ("qianfan", "qianfan"),
            ("ark", "ark"),
            ("siliconflow", "siliconflow"),
            ("ollama", "ollama"),
        ];

        for (provider, expected_provider_key) in cases {
            let id = integration_id_from_provider(provider)
                .expect("provider should map to dashboard integration id");
            assert_eq!(
                provider_key_from_integration_id(&id),
                Some(expected_provider_key),
                "provider '{provider}' with id '{id}' should resolve to '{expected_provider_key}'",
            );
        }
    }
}
