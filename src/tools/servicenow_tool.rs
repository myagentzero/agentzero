use super::traits::{Tool, ToolResult};
use crate::security::{SecurityPolicy, policy::ToolOperation};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const MAX_ERROR_BODY_CHARS: usize = 500;
const TOKEN_BUFFER_SECS: u64 = 30;
const DEFAULT_QUERY_LIMIT: u32 = 25;
const MAX_QUERY_LIMIT: u32 = 1000;

/// Tool for interacting with the ServiceNow Table API using OAuth2 client
/// credentials.
///
/// Actions are gated by `[servicenow].allowed_actions` and the security
/// policy's Read/Act split:
/// - `list_records`, `get_record` — read-only.
/// - `create_record`, `update_record` — mutating (Act policy).
///
/// Tokens are cached in-memory until shortly before expiry; no on-disk
/// caching is performed.
pub struct ServiceNowTool {
    base_url: String,
    client_id: String,
    client_secret: String,
    allowed_actions: Vec<String>,
    http: Client,
    security: Arc<SecurityPolicy>,
    timeout_secs: u64,
    token_cache: Mutex<Option<CachedToken>>,
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: u64,
}

impl ServiceNowTool {
    pub fn new(
        base_url: String,
        client_id: String,
        client_secret: String,
        allowed_actions: Vec<String>,
        security: Arc<SecurityPolicy>,
        timeout_secs: u64,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client_id,
            client_secret,
            allowed_actions,
            http: Client::new(),
            security,
            timeout_secs,
            token_cache: Mutex::new(None),
        }
    }

    fn is_action_allowed(&self, action: &str) -> bool {
        self.allowed_actions.iter().any(|a| a == action)
    }

    fn now_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    async fn get_access_token(&self) -> anyhow::Result<String> {
        let now = Self::now_unix_secs();

        {
            let cached = self.token_cache.lock().await;
            if let Some(cached) = cached.as_ref() {
                if now < cached.expires_at {
                    return Ok(cached.token.clone());
                }
            }
        }

        let url = format!("{}/oauth_token.do", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .timeout(Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ServiceNow token request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "ServiceNow token request failed ({status}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse ServiceNow token response: {e}"))?;

        let token = body["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("ServiceNow token response missing access_token"))?
            .to_string();
        let expires_in = body["expires_in"].as_u64().unwrap_or(1800);
        let expires_at = now + expires_in.saturating_sub(TOKEN_BUFFER_SECS);

        let mut cached = self.token_cache.lock().await;
        *cached = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });
        Ok(token)
    }

    async fn list_records(
        &self,
        table: &str,
        query: Option<&str>,
        fields: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<ToolResult> {
        validate_table(table)?;
        let token = self.get_access_token().await?;
        let url = format!("{}/api/now/table/{}", self.base_url, table);

        let limit_str = limit.clamp(1, MAX_QUERY_LIMIT).to_string();
        let mut query_params: Vec<(&str, &str)> = vec![("sysparm_limit", limit_str.as_str())];
        if let Some(q) = query {
            query_params.push(("sysparm_query", q));
        }
        if let Some(f) = fields {
            query_params.push(("sysparm_fields", f));
        }

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .bearer_auth(&token)
            .query(&query_params)
            .timeout(Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ServiceNow list_records request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "ServiceNow list_records failed ({status}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse ServiceNow list response: {e}"))?;

        let result = body.get("result").cloned().unwrap_or(json!([]));
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
            error: None,
        })
    }

    async fn get_record(
        &self,
        table: &str,
        sys_id: &str,
        fields: Option<&str>,
    ) -> anyhow::Result<ToolResult> {
        validate_table(table)?;
        validate_sys_id(sys_id)?;

        let token = self.get_access_token().await?;
        let url = format!("{}/api/now/table/{}/{}", self.base_url, table, sys_id);

        let mut query_params: Vec<(&str, &str)> = Vec::new();
        if let Some(f) = fields {
            query_params.push(("sysparm_fields", f));
        }

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .bearer_auth(&token)
            .query(&query_params)
            .timeout(Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ServiceNow get_record request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "ServiceNow get_record failed ({status}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse ServiceNow record response: {e}"))?;

        let result = body.get("result").cloned().unwrap_or(json!({}));
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
            error: None,
        })
    }

    async fn create_record(&self, table: &str, body: &Value) -> anyhow::Result<ToolResult> {
        validate_table(table)?;
        if !body.is_object() {
            anyhow::bail!("create_record requires `body` to be a JSON object");
        }

        let token = self.get_access_token().await?;
        let url = format!("{}/api/now/table/{}", self.base_url, table);

        let resp = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .bearer_auth(&token)
            .json(body)
            .timeout(Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ServiceNow create_record request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "ServiceNow create_record failed ({status}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let response: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse ServiceNow create response: {e}"))?;

        let result = response.get("result").cloned().unwrap_or(json!({}));
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
            error: None,
        })
    }

    async fn update_record(
        &self,
        table: &str,
        sys_id: &str,
        body: &Value,
    ) -> anyhow::Result<ToolResult> {
        validate_table(table)?;
        validate_sys_id(sys_id)?;
        if !body.is_object() {
            anyhow::bail!("update_record requires `body` to be a JSON object");
        }

        let token = self.get_access_token().await?;
        let url = format!("{}/api/now/table/{}/{}", self.base_url, table, sys_id);

        let resp = self
            .http
            .patch(&url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .bearer_auth(&token)
            .json(body)
            .timeout(Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ServiceNow update_record request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "ServiceNow update_record failed ({status}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let response: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse ServiceNow update response: {e}"))?;

        let result = response.get("result").cloned().unwrap_or(json!({}));
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
            error: None,
        })
    }
}

#[async_trait]
impl Tool for ServiceNowTool {
    fn name(&self) -> &str {
        "servicenow"
    }

    fn description(&self) -> &str {
        "ServiceNow Table API. \
         Common records: incident (INC), change (CHG), problem (PRB), vulnerability (VUL), sc_request (REQ), task (TASK), cmdb_ci (CI)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_records", "get_record", "create_record", "update_record"],
                    "description": "ServiceNow action to perform"
                },
                "table": {
                    "type": "string",
                    "description": "ServiceNow table name (e.g. incident, change_request, problem, sn_vul_vulnerable_item, sc_request, task, cmdb_ci)"
                },
                "sys_id": {
                    "type": "string",
                    "description": "Record sys_id (32-char hex). Required for get_record and update_record."
                },
                "query": {
                    "type": "string",
                    "description": "Optional sysparm_query encoded query for list_records (e.g. 'state=1^priority=1')."
                },
                "fields": {
                    "type": "string",
                    "description": "Optional comma-separated field allowlist (sysparm_fields)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max rows for list_records. Defaults to 25, capped at 1000.",
                    "default": 25
                },
                "body": {
                    "type": "object",
                    "description": "JSON body for create_record / update_record (field name → value).",
                    "additionalProperties": true
                }
            },
            "required": ["action", "table"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter: action".into()),
                });
            }
        };

        if !matches!(
            action,
            "list_records" | "get_record" | "create_record" | "update_record"
        ) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Unknown action: '{action}'. Valid actions: list_records, get_record, create_record, update_record"
                )),
            });
        }

        if !self.is_action_allowed(action) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Action '{action}' is not enabled. Add it to servicenow.allowed_actions in config.toml. \
                     Currently allowed: {}",
                    self.allowed_actions.join(", ")
                )),
            });
        }

        let operation = match action {
            "list_records" | "get_record" => ToolOperation::Read,
            "create_record" | "update_record" => ToolOperation::Act,
            _ => unreachable!(),
        };
        if let Err(error) = self
            .security
            .enforce_tool_operation(operation, "servicenow")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let table = match args.get("table").and_then(|v| v.as_str()) {
            Some(t) if !t.trim().is_empty() => t,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter: table".into()),
                });
            }
        };

        let result = match action {
            "list_records" => {
                let query = args.get("query").and_then(|v| v.as_str());
                let fields = args.get("fields").and_then(|v| v.as_str());
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|n| u32::try_from(n).unwrap_or(MAX_QUERY_LIMIT))
                    .unwrap_or(DEFAULT_QUERY_LIMIT);
                self.list_records(table, query, fields, limit).await
            }
            "get_record" => {
                let sys_id = match args.get("sys_id").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("get_record requires sys_id parameter".into()),
                        });
                    }
                };
                let fields = args.get("fields").and_then(|v| v.as_str());
                self.get_record(table, sys_id, fields).await
            }
            "create_record" => {
                let body = match args.get("body") {
                    Some(b) if b.is_object() => b,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "create_record requires `body` parameter (JSON object)".into(),
                            ),
                        });
                    }
                };
                self.create_record(table, body).await
            }
            "update_record" => {
                let sys_id = match args.get("sys_id").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("update_record requires sys_id parameter".into()),
                        });
                    }
                };
                let body = match args.get("body") {
                    Some(b) if b.is_object() => b,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "update_record requires `body` parameter (JSON object)".into(),
                            ),
                        });
                    }
                };
                self.update_record(table, sys_id, body).await
            }
            _ => unreachable!(),
        };

        match result {
            Ok(tool_result) => Ok(tool_result),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}

// ── Input validation ─────────────────────────────────────────────────────────

/// Restrict the table name to ServiceNow's documented format
/// (lowercase letters, digits, underscore) to prevent path traversal when the
/// value is interpolated directly into the URL.
fn validate_table(table: &str) -> anyhow::Result<()> {
    let valid = !table.is_empty()
        && table
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        anyhow::bail!(
            "Invalid table name '{table}'. Expected lowercase letters, digits, or underscores."
        )
    }
}

/// ServiceNow sys_ids are 32-character hex strings.
fn validate_sys_id(sys_id: &str) -> anyhow::Result<()> {
    let valid = sys_id.len() == 32 && sys_id.chars().all(|c| c.is_ascii_hexdigit());
    if valid {
        Ok(())
    } else {
        anyhow::bail!("Invalid sys_id '{sys_id}'. Expected a 32-character hex string.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;
    use crate::security::policy::AutonomyLevel;

    fn test_tool(allowed_actions: Vec<&str>) -> ServiceNowTool {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            ..SecurityPolicy::default()
        });
        ServiceNowTool::new(
            "https://test.service-now.com".into(),
            "client-id".into(),
            "client-secret".into(),
            allowed_actions.into_iter().map(String::from).collect(),
            security,
            30,
        )
    }

    #[test]
    fn tool_name_is_servicenow() {
        assert_eq!(test_tool(vec!["get_record"]).name(), "servicenow");
    }

    #[test]
    fn parameters_schema_has_required_fields() {
        let schema = test_tool(vec!["get_record"]).parameters_schema();
        let required = schema["required"].as_array().unwrap();
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"action"));
        assert!(names.contains(&"table"));
    }

    #[test]
    fn parameters_schema_lists_all_actions() {
        let schema = test_tool(vec!["get_record"]).parameters_schema();
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        let names: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"list_records"));
        assert!(names.contains(&"get_record"));
        assert!(names.contains(&"create_record"));
        assert!(names.contains(&"update_record"));
    }

    #[tokio::test]
    async fn execute_missing_action_returns_error() {
        let result = test_tool(vec!["get_record"])
            .execute(json!({}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("action"));
    }

    #[tokio::test]
    async fn execute_unknown_action_returns_error() {
        let result = test_tool(vec!["get_record"])
            .execute(json!({"action": "delete_record", "table": "incident"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("Unknown action"));
    }

    #[tokio::test]
    async fn execute_disallowed_action_returns_error() {
        let result = test_tool(vec!["get_record"])
            .execute(json!({"action": "create_record", "table": "incident", "body": {}}))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap();
        assert!(err.contains("not enabled"));
        assert!(err.contains("allowed_actions"));
    }

    #[tokio::test]
    async fn execute_missing_table_returns_error() {
        let result = test_tool(vec!["list_records"])
            .execute(json!({"action": "list_records"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("table"));
    }

    #[tokio::test]
    async fn execute_get_record_missing_sys_id_returns_error() {
        let result = test_tool(vec!["get_record"])
            .execute(json!({"action": "get_record", "table": "incident"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("sys_id"));
    }

    #[tokio::test]
    async fn execute_create_record_missing_body_returns_error() {
        let result = test_tool(vec!["create_record"])
            .execute(json!({"action": "create_record", "table": "incident"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("body"));
    }

    #[tokio::test]
    async fn execute_update_record_blocked_in_readonly_mode() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::ReadOnly,
            ..SecurityPolicy::default()
        });
        let tool = ServiceNowTool::new(
            "https://test.service-now.com".into(),
            "id".into(),
            "secret".into(),
            vec!["update_record".into()],
            security,
            30,
        );
        let result = tool
            .execute(json!({
                "action": "update_record",
                "table": "incident",
                "sys_id": "0123456789abcdef0123456789abcdef",
                "body": {"state": "2"}
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("read-only"));
    }

    #[test]
    fn validate_table_accepts_known_tables() {
        assert!(validate_table("incident").is_ok());
        assert!(validate_table("change_request").is_ok());
        assert!(validate_table("cmdb_ci").is_ok());
        assert!(validate_table("sc_request").is_ok());
    }

    #[test]
    fn validate_table_rejects_path_traversal() {
        assert!(validate_table("../../etc/passwd").is_err());
        assert!(validate_table("incident/../change_request").is_err());
        assert!(validate_table("incident?sysparm").is_err());
        assert!(validate_table("INCIDENT").is_err());
        assert!(validate_table("").is_err());
    }

    #[test]
    fn validate_sys_id_accepts_32_hex_chars() {
        assert!(validate_sys_id("0123456789abcdef0123456789abcdef").is_ok());
        assert!(validate_sys_id("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").is_ok());
    }

    #[test]
    fn validate_sys_id_rejects_invalid() {
        assert!(validate_sys_id("").is_err());
        assert!(validate_sys_id("short").is_err());
        assert!(validate_sys_id("01234567-89ab-cdef-0123-456789abcdef").is_err());
        assert!(validate_sys_id("zzzz5678901234567890123456789012").is_err());
    }
}
