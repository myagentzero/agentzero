//! Provider status tool — queries a LiteLLM-compatible proxy's `/key/info`
//! endpoint to report API key spend, budget, and expiry.
//!
//! Only registered when the configured `default_provider` starts with `"custom:"`.

use super::traits::{Tool, ToolResult};
use crate::config::Config;
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT_SECS: u64 = 15;
const CONNECT_TIMEOUT_SECS: u64 = 10;

// ── API response types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct KeyInfoResponse {
    info: KeyInfo,
}

#[derive(Debug, Deserialize)]
struct KeyInfo {
    key_alias: Option<String>,
    spend: Option<f64>,
    max_budget: Option<f64>,
    expires: Option<String>,
    budget_duration: Option<String>,
    budget_reset_at: Option<String>,
}

// ── Tool struct ────────────────────────────────────────────────────────────

pub struct ProviderStatusTool {
    config: Arc<Config>,
}

impl ProviderStatusTool {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }

    fn build_client() -> anyhow::Result<reqwest::Client> {
        let builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .user_agent(crate::config::schema::DEFAULT_USER_AGENT);

        let builder =
            crate::config::apply_runtime_proxy_to_builder(builder, "tool.provider_status");
        Ok(builder.build()?)
    }

    fn format_duration_until(target: DateTime<FixedOffset>, now: DateTime<Utc>) -> String {
        let diff = target.signed_duration_since(now);
        if diff.num_seconds() <= 0 {
            return "expired".to_string();
        }
        let days = diff.num_days();
        let hours = diff.num_hours() % 24;
        match (days, hours) {
            (0, h) => format!("in {h} hours"),
            (d, 0) => format!("in {d} days"),
            (d, h) => format!("in {d} days {h} hours"),
        }
    }

    fn format_output(info: &KeyInfo) -> String {
        let now = Utc::now();
        let mut out = String::from(
            "Provider Status (Custom Proxy)\n\
             ───────────────────────────────",
        );

        if let Some(alias) = &info.key_alias {
            out.push_str(&format!("\nKey:             {alias}"));
        }

        match (info.spend, info.max_budget) {
            (Some(spend), Some(budget)) if budget > 0.0 => {
                let pct = (spend / budget) * 100.0;
                let remaining = (budget - spend).max(0.0);
                out.push_str(&format!(
                    "\nSpend:           ${spend:.2} / ${budget:.2} ({pct:.1}%)"
                ));
                out.push_str(&format!("\nRemaining:       ${remaining:.2}"));
            }
            (Some(spend), Some(budget)) => {
                out.push_str(&format!("\nSpend:           ${spend:.2} / ${budget:.2}"));
                out.push_str("\nRemaining:       $0.00");
            }
            (Some(spend), None) => {
                out.push_str(&format!("\nSpend:           ${spend:.2} (no budget limit)"));
            }
            _ => {}
        }

        if let Some(duration) = &info.budget_duration {
            let reset_str = info
                .budget_reset_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| {
                    let until = Self::format_duration_until(dt, now);
                    format!(" (resets {} UTC — {until})", dt.format("%Y-%m-%d %H:%M"))
                })
                .unwrap_or_default();
            out.push_str(&format!("\nBudget period:   {duration}{reset_str}"));
        }

        if let Some(expires_str) = &info.expires {
            if let Ok(dt) = DateTime::parse_from_rfc3339(expires_str) {
                let until = Self::format_duration_until(dt, now);
                out.push_str(&format!(
                    "\nKey expires:     {} UTC ({until})",
                    dt.format("%Y-%m-%d %H:%M")
                ));
            }
        }

        out
    }
}

// ── Tool trait ─────────────────────────────────────────────────────────────

#[async_trait]
impl Tool for ProviderStatusTool {
    fn name(&self) -> &str {
        "provider_status"
    }

    fn description(&self) -> &str {
        "Check the current API key's spend, budget, and expiry against the LiteLLM proxy. Use when: user asks about remaining budget, spend, or key expiry."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        let api_url = match self.config.api_url.as_deref() {
            Some(url) if !url.is_empty() => url,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(
                        "No api_url configured. Set api_url in config.toml for the custom provider."
                            .into(),
                    ),
                });
            }
        };

        let api_key = match self.config.api_key.as_deref() {
            Some(key) if !key.is_empty() => key,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(
                        "No api_key configured. Set api_key in config.toml or ZEROCLAW_API_KEY env var."
                            .into(),
                    ),
                });
            }
        };

        let client = match Self::build_client() {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to build HTTP client: {e}")),
                });
            }
        };

        let url = format!("{}/key/info", api_url.trim_end_matches('/'));
        let response = match client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to reach provider proxy: {e}")),
                });
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Provider proxy returned HTTP {status}. Response: {body}"
                )),
            });
        }

        let key_info: KeyInfoResponse = match response.json().await {
            Ok(parsed) => parsed,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to parse /key/info response: {e}")),
                });
            }
        };

        let output = Self::format_output(&key_info.info);
        tracing::info!(
            spend = key_info.info.spend,
            max_budget = key_info.info.max_budget,
            key_alias = key_info.info.key_alias.as_deref().unwrap_or("N/A"),
            "provider status fetched"
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> ProviderStatusTool {
        ProviderStatusTool::new(Arc::new(Config::default()))
    }

    fn make_full_info() -> KeyInfo {
        KeyInfo {
            key_alias: Some("bxfocht-march".into()),
            spend: Some(211.54),
            max_budget: Some(300.0),
            expires: Some("2099-04-01T16:40:12.784000+00:00".into()),
            budget_duration: Some("30d".into()),
            budget_reset_at: Some("2099-04-01T00:00:00+00:00".into()),
        }
    }

    // ── Metadata ────────────────────────────────────────────────────────

    #[test]
    fn name_is_provider_status() {
        assert_eq!(make_tool().name(), "provider_status");
    }

    #[test]
    fn description_is_non_empty() {
        assert!(!make_tool().description().is_empty());
    }

    #[test]
    fn parameters_schema_is_valid_empty_object() {
        let schema = make_tool().parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].is_object());
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn spec_reflects_tool_metadata() {
        let tool = make_tool();
        let spec = tool.spec();
        assert_eq!(spec.name, "provider_status");
        assert_eq!(spec.description, tool.description());
        assert!(spec.parameters.is_object());
    }

    // ── JSON deserialization ────────────────────────────────────────────

    #[test]
    fn deserialize_full_response() {
        let json_str = r#"{
            "key": "abc123",
            "info": {
                "key_alias": "bxfocht-march",
                "spend": 211.54,
                "max_budget": 300.0,
                "expires": "2026-04-01T16:40:12.784000+00:00",
                "budget_duration": "30d",
                "budget_reset_at": "2026-04-01T00:00:00+00:00"
            }
        }"#;
        let parsed: KeyInfoResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.info.key_alias.as_deref(), Some("bxfocht-march"));
        assert!((parsed.info.spend.unwrap() - 211.54).abs() < 0.01);
        assert!((parsed.info.max_budget.unwrap() - 300.0).abs() < 0.01);
        assert!(parsed.info.expires.is_some());
        assert_eq!(parsed.info.budget_duration.as_deref(), Some("30d"));
    }

    #[test]
    fn deserialize_response_with_nulls() {
        let json_str = r#"{
            "key": "abc123",
            "info": {
                "key_alias": null,
                "spend": null,
                "max_budget": null,
                "expires": null,
                "budget_duration": null,
                "budget_reset_at": null
            }
        }"#;
        let parsed: KeyInfoResponse = serde_json::from_str(json_str).unwrap();
        assert!(parsed.info.key_alias.is_none());
        assert!(parsed.info.spend.is_none());
        assert!(parsed.info.max_budget.is_none());
    }

    #[test]
    fn deserialize_response_missing_fields() {
        let json_str = r#"{"key": "abc123", "info": {}}"#;
        let parsed: KeyInfoResponse = serde_json::from_str(json_str).unwrap();
        assert!(parsed.info.key_alias.is_none());
        assert!(parsed.info.spend.is_none());
    }

    // ── format_output ───────────────────────────────────────────────────

    #[test]
    fn format_output_full() {
        let info = make_full_info();
        let out = ProviderStatusTool::format_output(&info);
        assert!(out.contains("bxfocht-march"));
        assert!(out.contains("$211.54"));
        assert!(out.contains("$300.00"));
        assert!(out.contains("70.5%"));
        assert!(out.contains("$88.46"));
        assert!(out.contains("30d"));
        assert!(out.contains("2099-04-01"));
    }

    #[test]
    fn format_output_partial() {
        let info = KeyInfo {
            key_alias: None,
            spend: Some(50.0),
            max_budget: None,
            expires: None,
            budget_duration: None,
            budget_reset_at: None,
        };
        let out = ProviderStatusTool::format_output(&info);
        assert!(out.contains("$50.00"));
        assert!(out.contains("no budget limit"));
        assert!(!out.contains("Key:"));
        assert!(!out.contains("expires"));
    }

    #[test]
    fn format_output_zero_budget() {
        let info = KeyInfo {
            key_alias: None,
            spend: Some(0.0),
            max_budget: Some(0.0),
            expires: None,
            budget_duration: None,
            budget_reset_at: None,
        };
        let out = ProviderStatusTool::format_output(&info);
        assert!(out.contains("$0.00 / $0.00"));
        assert!(out.contains("Remaining:       $0.00"));
    }

    #[test]
    fn format_output_expired_key() {
        let info = KeyInfo {
            key_alias: None,
            spend: None,
            max_budget: None,
            expires: Some("2020-01-01T00:00:00+00:00".into()),
            budget_duration: None,
            budget_reset_at: None,
        };
        let out = ProviderStatusTool::format_output(&info);
        assert!(out.contains("expired"));
    }

    // ── format_duration_until ───────────────────────────────────────────

    #[test]
    fn duration_until_future() {
        let now = Utc::now();
        let target = DateTime::parse_from_rfc3339("2099-12-31T23:59:59+00:00").unwrap();
        let result = ProviderStatusTool::format_duration_until(target, now);
        assert!(result.starts_with("in "));
        assert!(result.contains("days"));
    }

    #[test]
    fn duration_until_past() {
        let now = Utc::now();
        let target = DateTime::parse_from_rfc3339("2020-01-01T00:00:00+00:00").unwrap();
        let result = ProviderStatusTool::format_duration_until(target, now);
        assert_eq!(result, "expired");
    }

    // ── execute: missing config ─────────────────────────────────────────

    #[tokio::test]
    async fn execute_returns_error_when_no_api_url() {
        let config = Config {
            api_url: None,
            api_key: Some("sk-test".into()),
            ..Config::default()
        };
        let tool = ProviderStatusTool::new(Arc::new(config));
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("api_url"));
    }

    #[tokio::test]
    async fn execute_returns_error_when_no_api_key() {
        let config = Config {
            api_url: Some("https://example.com".into()),
            api_key: None,
            ..Config::default()
        };
        let tool = ProviderStatusTool::new(Arc::new(config));
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("api_key"));
    }
}
