use super::traits::{Tool, ToolResult};
use crate::security::{SecurityPolicy, policy::ToolOperation};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::Arc;

const CONFLUENCE_SEARCH_PAGE_SIZE: u32 = 100;
const MAX_ERROR_BODY_CHARS: usize = 500;

/// Controls how much data is returned by `get_page`.
#[derive(Default)]
enum ContentDetailLevel {
    Basic,
    #[default]
    Standard,
    Full,
}

impl ContentDetailLevel {
    fn from_str(s: &str) -> Self {
        match s {
            "basic" => Self::Basic,
            "full" => Self::Full,
            _ => Self::Standard,
        }
    }
}

/// Tool for interacting with the Confluence REST API v2.
///
/// Supports four read-only actions gated by `[confluence].allowed_actions` in config:
/// - `get_page`      — always in the default allowlist; read-only.
/// - `search_pages`  — always in the default allowlist; read-only.
/// - `list_spaces`   — requires explicit opt-in; read-only.
/// - `get_space`     — requires explicit opt-in; read-only.
pub struct ConfluenceTool {
    base_url: String,
    email: String,
    api_token: String,
    allowed_actions: Vec<String>,
    http: Client,
    security: Arc<SecurityPolicy>,
    timeout_secs: u64,
}

impl ConfluenceTool {
    pub fn new(
        base_url: String,
        email: String,
        api_token: String,
        allowed_actions: Vec<String>,
        security: Arc<SecurityPolicy>,
        timeout_secs: u64,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            email,
            api_token,
            allowed_actions,
            http: Client::new(),
            security,
            timeout_secs,
        }
    }

    fn is_action_allowed(&self, action: &str) -> bool {
        self.allowed_actions.iter().any(|a| a == action)
    }

    async fn get_page(
        &self,
        page_id: &str,
        level: ContentDetailLevel,
    ) -> anyhow::Result<ToolResult> {
        validate_page_id(page_id)?;
        let url = format!("{}/wiki/api/v2/pages/{}", self.base_url, page_id);

        let query_params: Vec<(&str, &str)> = match &level {
            ContentDetailLevel::Basic => {
                vec![("include-labels", "false"), ("include-version", "true")]
            }
            ContentDetailLevel::Standard => vec![
                ("include-labels", "true"),
                ("include-version", "true"),
                ("body-format", "view"),
            ],
            ContentDetailLevel::Full => vec![
                ("include-labels", "true"),
                ("include-version", "true"),
                ("include-properties", "true"),
                ("body-format", "view"),
            ],
        };

        let resp = self
            .http
            .get(&url)
            .basic_auth(&self.email, Some(&self.api_token))
            .query(&query_params)
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Confluence get_page request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Confluence get_page failed ({status}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let raw: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse Confluence get_page response: {e}"))?;

        let shaped = match level {
            ContentDetailLevel::Basic => shape_page_basic(&raw),
            ContentDetailLevel::Standard => shape_page_standard(&raw),
            ContentDetailLevel::Full => shape_page_full(&raw),
        };

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&shaped).unwrap_or_else(|_| shaped.to_string()),
            error: None,
        })
    }

    #[allow(clippy::cast_possible_truncation)]
    async fn search_pages(
        &self,
        cql: &str,
        max_results: Option<u32>,
    ) -> anyhow::Result<ToolResult> {
        validate_cql(cql)?;

        let max_results = max_results.unwrap_or(25).clamp(1, 999);
        let mut pages: Vec<Value> = Vec::new();
        let mut start: u32 = 0;

        loop {
            let remaining = max_results.saturating_sub(pages.len() as u32);
            let page_size = remaining.min(CONFLUENCE_SEARCH_PAGE_SIZE);

            let cql_param = cql.to_string();
            let limit_str = page_size.to_string();
            let start_str = start.to_string();
            let query_params = vec![
                ("cql", cql_param.as_str()),
                ("limit", limit_str.as_str()),
                ("start", start_str.as_str()),
                ("expand", "version,space,history"),
            ];

            let url = format!("{}/wiki/rest/api/content/search", self.base_url);
            let resp = self
                .http
                .get(&url)
                .basic_auth(&self.email, Some(&self.api_token))
                .query(&query_params)
                .timeout(std::time::Duration::from_secs(self.timeout_secs))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Confluence search_pages request failed: {e}"))?;

            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!(
                    "Confluence search_pages failed ({status}): {}",
                    crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
                );
            }

            let raw: Value = resp
                .json()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to parse Confluence search response: {e}"))?;

            let batch_size = raw["results"].as_array().map_or(0, |a| a.len());
            if let Some(results) = raw["results"].as_array() {
                pages.extend(results.iter().map(shape_search_result));
            }

            if pages.len() as u32 >= max_results || batch_size == 0 {
                break;
            }

            start += batch_size as u32;

            if raw["_links"]["next"].is_null() {
                break;
            }
        }

        let output = json!(pages);
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
            error: None,
        })
    }

    async fn list_spaces(
        &self,
        max_results: Option<u32>,
        space_type: Option<&str>,
        status: Option<&str>,
    ) -> anyhow::Result<ToolResult> {
        let max_results = max_results.unwrap_or(50).clamp(1, 999);
        let url = format!("{}/wiki/api/v2/spaces", self.base_url);

        let limit_str = max_results.to_string();
        let mut query_params = vec![("limit", limit_str.as_str())];

        if let Some(t) = space_type {
            query_params.push(("type", t));
        }
        if let Some(s) = status {
            query_params.push(("status", s));
        }

        let resp = self
            .http
            .get(&url)
            .basic_auth(&self.email, Some(&self.api_token))
            .query(&query_params)
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Confluence list_spaces request failed: {e}"))?;

        let status_code = resp.status();
        if !status_code.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Confluence list_spaces failed ({status_code}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let raw: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse Confluence list_spaces response: {e}"))?;

        let spaces: Vec<Value> = raw["results"]
            .as_array()
            .map(|arr| arr.iter().map(shape_space).collect())
            .unwrap_or_default();

        let output = json!({ "spaces": spaces });
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string()),
            error: None,
        })
    }

    async fn get_space(&self, space_key: &str, include_pages: bool) -> anyhow::Result<ToolResult> {
        validate_space_key(space_key)?;

        let url = format!("{}/wiki/api/v2/spaces/{}", self.base_url, space_key);

        let resp = self
            .http
            .get(&url)
            .basic_auth(&self.email, Some(&self.api_token))
            .query(&[("description-format", "view")])
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Confluence get_space request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Confluence get_space failed ({status}): {}",
                crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
            );
        }

        let space_raw: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse Confluence get_space response: {e}"))?;

        let mut result = shape_space(&space_raw);

        if include_pages {
            let pages_url = format!("{}/wiki/api/v2/spaces/{}/pages", self.base_url, space_key);
            let pages_resp = self
                .http
                .get(&pages_url)
                .basic_auth(&self.email, Some(&self.api_token))
                .query(&[("limit", "25")])
                .timeout(std::time::Duration::from_secs(self.timeout_secs))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Confluence get_space pages request failed: {e}"))?;

            if pages_resp.status().is_success() {
                if let Ok(pages_raw) = pages_resp.json::<Value>().await {
                    let pages: Vec<Value> = pages_raw["results"]
                        .as_array()
                        .map(|arr| arr.iter().map(shape_page_basic).collect())
                        .unwrap_or_default();
                    result["recentPages"] = json!(pages);
                }
            }
        }

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
            error: None,
        })
    }
}

#[async_trait]
impl Tool for ConfluenceTool {
    fn name(&self) -> &str {
        "confluence"
    }

    fn description(&self) -> &str {
        "Interact with Confluence: read documentation pages, search with CQL, explore spaces. \
         Use when: user asks about work related documentation or references Confluence pages/spaces. \
         Don't use when: user is discussing documentation conceptually or when referencing content from the web."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get_page", "search_pages", "list_spaces", "get_space"],
                    "description": "The Confluence action to perform. Enabled actions are configured in [confluence].allowed_actions."
                },
                "page_id": {
                    "type": "string",
                    "description": "Numeric Confluence page ID (e.g., '12345'). Required for get_page."
                },
                "detail_level": {
                    "type": "string",
                    "enum": ["basic", "standard", "full"],
                    "description": "How much content to return for get_page. Options: 'basic' — minimal metadata only (title, space, version); 'standard' (default) — includes excerpt and labels; 'full' — complete page body content."
                },
                "cql": {
                    "type": "string",
                    "description": "CQL (Confluence Query Language) query string for search_pages. Example: 'space=DEV AND type=page AND title~\"API\"'. Common fields: space, type, title, creator, created, lastModified. Operators: =, !=, ~, >, <, AND, OR."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return for search_pages or list_spaces. Defaults to 25 for search, 50 for list_spaces. Capped at 999.",
                    "default": 25
                },
                "space_key": {
                    "type": "string",
                    "description": "Confluence space key (e.g., 'PROJ', 'DEV') or numeric space ID. Required for get_space."
                },
                "include_pages": {
                    "type": "boolean",
                    "description": "Whether to include a list of recent pages when calling get_space. Defaults to true.",
                    "default": true
                },
                "type": {
                    "type": "string",
                    "enum": ["global", "personal"],
                    "description": "Filter list_spaces by space type. Omit to return all types."
                },
                "status": {
                    "type": "string",
                    "enum": ["current", "archived"],
                    "description": "Filter list_spaces by space status. Defaults to 'current'."
                }
            },
            "required": ["action"]
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

        // Reject unknown actions before the allowlist check
        if !matches!(
            action,
            "get_page" | "search_pages" | "list_spaces" | "get_space"
        ) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Unknown action: '{action}'. Valid actions: get_page, search_pages, list_spaces, get_space"
                )),
            });
        }

        if !self.is_action_allowed(action) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Action '{action}' is not enabled. Add it to confluence.allowed_actions in config.toml. \
                     Currently allowed: {}",
                    self.allowed_actions.join(", ")
                )),
            });
        }

        let operation = match action {
            "get_page" | "search_pages" | "list_spaces" | "get_space" => ToolOperation::Read,
            _ => unreachable!(),
        };

        if let Err(error) = self
            .security
            .enforce_tool_operation(operation, "confluence")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let result = match action {
            "get_page" => {
                let page_id = match args.get("page_id").and_then(|v| v.as_str()) {
                    Some(k) => k,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("get_page requires page_id parameter".into()),
                        });
                    }
                };
                let level = args
                    .get("detail_level")
                    .and_then(|v| v.as_str())
                    .map(ContentDetailLevel::from_str)
                    .unwrap_or_default();
                self.get_page(page_id, level).await
            }
            "search_pages" => {
                let cql = match args.get("cql").and_then(|v| v.as_str()) {
                    Some(c) => c,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("search_pages requires cql parameter".into()),
                        });
                    }
                };
                let max_results = args
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .map(|n| u32::try_from(n).unwrap_or(u32::MAX));
                self.search_pages(cql, max_results).await
            }
            "list_spaces" => {
                let max_results = args
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .map(|n| u32::try_from(n).unwrap_or(u32::MAX));
                let space_type = args.get("type").and_then(|v| v.as_str());
                let status = args.get("status").and_then(|v| v.as_str());
                self.list_spaces(max_results, space_type, status).await
            }
            "get_space" => {
                let space_key = match args.get("space_key").and_then(|v| v.as_str()) {
                    Some(k) => k,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("get_space requires space_key parameter".into()),
                        });
                    }
                };
                let include_pages = args
                    .get("include_pages")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                self.get_space(space_key, include_pages).await
            }
            _ => unreachable!(),
        };

        result.or_else(|err| {
            Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(err.to_string()),
            })
        })
    }
}

// ── Input validation ──────────────────────────────────────────────────────────

fn validate_page_id(id: &str) -> anyhow::Result<()> {
    // Confluence page IDs are numeric strings
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        anyhow::bail!("Invalid page ID '{id}'. Expected numeric ID (e.g., '12345')")
    }
    Ok(())
}

fn validate_space_key(key: &str) -> anyhow::Result<()> {
    // Space keys are alphanumeric (e.g., PROJ, DEV) or numeric IDs
    let valid = !key.is_empty()
        && (key.chars().all(|c| c.is_ascii_alphanumeric())
            || key.chars().all(|c| c.is_ascii_digit()));

    if !valid {
        anyhow::bail!(
            "Invalid space key '{key}'. Expected alphanumeric key (e.g., 'PROJ') or numeric ID"
        )
    }
    Ok(())
}

fn validate_cql(cql: &str) -> anyhow::Result<()> {
    if cql.is_empty() {
        anyhow::bail!("CQL query cannot be empty")
    }
    if cql.len() > 2000 {
        anyhow::bail!("CQL query too long (max 2000 chars)")
    }
    Ok(())
}

// ── Response shaping ──────────────────────────────────────────────────────────

/// Safely extracts the first 10 characters (date prefix) from a string.
/// Returns the full string if it is shorter than 10 characters.
fn date_prefix(s: &str) -> &str {
    s.get(..10).unwrap_or(s)
}

fn extract_excerpt(raw: &Value) -> Value {
    // Try multiple possible fields for excerpt/summary
    if let Some(excerpt) = raw["excerpt"].as_str() {
        return json!(excerpt);
    }
    if let Some(body) = raw["body"]["view"]["value"].as_str() {
        // Truncate body to first 200 chars as excerpt
        let truncated = body.chars().take(200).collect::<String>();
        return json!(truncated);
    }
    Value::Null
}

fn extract_labels(raw: &Value) -> Value {
    if let Some(labels_array) = raw["labels"]["results"].as_array() {
        let labels: Vec<String> = labels_array
            .iter()
            .filter_map(|l| l["name"].as_str().map(String::from))
            .collect();
        return json!(labels);
    }
    json!([])
}

fn extract_body(raw: &Value) -> Value {
    // Try to get the rendered view format first, fallback to storage
    if let Some(view_body) = raw["body"]["view"]["value"].as_str() {
        return json!(view_body);
    }
    if let Some(storage_body) = raw["body"]["storage"]["value"].as_str() {
        return json!(storage_body);
    }
    Value::Null
}

fn shape_search_result(raw: &Value) -> Value {
    json!({
        "id": raw["id"],
        "title": raw["title"],
        "type": raw["type"],
        "status": raw["status"],
        "spaceKey": raw["space"]["key"],
        "version": raw["version"]["number"],
        "createdAt": date_prefix(raw["history"]["createdDate"].as_str().unwrap_or("")),
        "lastModified": date_prefix(raw["version"]["when"].as_str().unwrap_or("")),
        "authorId": raw["history"]["createdBy"]["accountId"],
        "excerpt": raw["excerpt"],
        "url": raw["_links"]["webui"],
    })
}

fn shape_page_basic(raw: &Value) -> Value {
    json!({
        "id": raw["id"],
        "title": raw["title"],
        "type": raw["type"],
        "status": raw["status"],
        "spaceId": raw["spaceId"],
        "version": raw["version"]["number"],
        "createdAt": date_prefix(raw["createdAt"].as_str().unwrap_or("")),
        "lastModified": date_prefix(raw["lastModified"].as_str().unwrap_or("")),
        "authorId": raw["authorId"],
        "url": raw["_links"]["webui"],
    })
}

fn shape_page_standard(raw: &Value) -> Value {
    let mut result = shape_page_basic(raw);
    let obj = result.as_object_mut().unwrap();
    obj.insert("excerpt".to_string(), extract_excerpt(raw));
    obj.insert("labels".to_string(), extract_labels(raw));
    result
}

fn shape_page_full(raw: &Value) -> Value {
    let mut result = shape_page_standard(raw);
    let obj = result.as_object_mut().unwrap();
    obj.insert("body".to_string(), extract_body(raw));
    result
}

fn shape_space(raw: &Value) -> Value {
    json!({
        "id": raw["id"],
        "key": raw["key"],
        "name": raw["name"],
        "type": raw["type"],
        "status": raw["status"],
        "description": raw["description"]["view"]["value"],
        "homepageId": raw["homepageId"],
        "url": raw["_links"]["webui"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;
    use crate::security::policy::AutonomyLevel;

    fn test_tool(allowed_actions: Vec<&str>) -> ConfluenceTool {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            ..SecurityPolicy::default()
        });
        ConfluenceTool::new(
            "https://test.atlassian.net".into(),
            "test@example.com".into(),
            "test-token".into(),
            allowed_actions.into_iter().map(String::from).collect(),
            security,
            30,
        )
    }

    #[test]
    fn tool_name_is_confluence() {
        assert_eq!(test_tool(vec!["get_page"]).name(), "confluence");
    }

    #[test]
    fn parameters_schema_has_required_action() {
        let schema = test_tool(vec!["get_page"]).parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("action")));
    }

    #[test]
    fn parameters_schema_defines_all_actions() {
        let schema = test_tool(vec!["get_page"]).parameters_schema();
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        let action_strs: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
        assert!(action_strs.contains(&"get_page"));
        assert!(action_strs.contains(&"search_pages"));
        assert!(action_strs.contains(&"list_spaces"));
        assert!(action_strs.contains(&"get_space"));
    }

    #[test]
    fn validate_page_id_accepts_numeric() {
        assert!(validate_page_id("12345").is_ok());
        assert!(validate_page_id("98765432").is_ok());
        assert!(validate_page_id("1").is_ok());
    }

    #[test]
    fn validate_page_id_rejects_invalid() {
        assert!(validate_page_id("").is_err());
        assert!(validate_page_id("abc").is_err());
        assert!(validate_page_id("123-456").is_err());
        assert!(validate_page_id("../etc/passwd").is_err());
        assert!(validate_page_id("12a34").is_err());
    }

    #[test]
    fn validate_space_key_accepts_valid() {
        assert!(validate_space_key("PROJ").is_ok());
        assert!(validate_space_key("DEV").is_ok());
        assert!(validate_space_key("ABC123").is_ok());
        assert!(validate_space_key("123456").is_ok()); // numeric space ID
    }

    #[test]
    fn validate_space_key_rejects_invalid() {
        assert!(validate_space_key("").is_err());
        assert!(validate_space_key("PROJ-123").is_err()); // hyphen not allowed
        assert!(validate_space_key("../other").is_err());
        assert!(validate_space_key("PROJ SPACE").is_err()); // space not allowed
    }

    #[test]
    fn validate_cql_accepts_valid() {
        assert!(validate_cql("space=PROJ").is_ok());
        assert!(validate_cql("space=DEV AND type=page").is_ok());
        assert!(validate_cql("title~\"API\" AND lastModified >= \"2024-01-01\"").is_ok());
    }

    #[test]
    fn validate_cql_rejects_invalid() {
        assert!(validate_cql("").is_err());
        assert!(validate_cql(&"x".repeat(2001)).is_err());
    }

    #[tokio::test]
    async fn execute_missing_action_returns_error() {
        let result = test_tool(vec!["get_page"])
            .execute(json!({}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("action"));
    }

    #[tokio::test]
    async fn execute_unknown_action_returns_error() {
        let result = test_tool(vec!["get_page"])
            .execute(json!({"action": "delete_page"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("Unknown action"));
    }

    #[tokio::test]
    async fn execute_disallowed_action_returns_error() {
        let result = test_tool(vec!["get_page"])
            .execute(json!({"action": "list_spaces"}))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap();
        assert!(err.contains("not enabled"));
        assert!(err.contains("allowed_actions"));
    }

    #[tokio::test]
    async fn execute_get_page_missing_id_returns_error() {
        let result = test_tool(vec!["get_page"])
            .execute(json!({"action": "get_page"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("page_id"));
    }

    #[tokio::test]
    async fn execute_search_pages_missing_cql_returns_error() {
        let result = test_tool(vec!["get_page", "search_pages"])
            .execute(json!({"action": "search_pages"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("cql"));
    }

    #[tokio::test]
    async fn execute_get_space_missing_key_returns_error() {
        let result = test_tool(vec!["get_space"])
            .execute(json!({"action": "get_space"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("space_key"));
    }

    #[tokio::test]
    async fn read_operations_not_blocked_in_readonly_mode() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::ReadOnly,
            ..SecurityPolicy::default()
        });
        let tool = ConfluenceTool::new(
            "https://127.0.0.1:1".into(),
            "test@example.com".into(),
            "token".into(),
            vec!["get_page".into()],
            security,
            30,
        );
        let result = tool
            .execute(json!({"action": "get_page", "page_id": "12345"}))
            .await
            .unwrap();
        assert!(!result.success); // Fails on network, not policy
        assert!(!result.error.as_deref().unwrap_or("").contains("read-only"));
    }

    #[test]
    fn shape_page_basic_extracts_expected_fields() {
        let raw = json!({
            "id": "12345",
            "title": "Architecture Guide",
            "type": "page",
            "status": "current",
            "spaceId": "67890",
            "version": { "number": 5 },
            "createdAt": "2024-01-15T10:00:00.000Z",
            "lastModified": "2024-03-01T12:00:00.000Z",
            "authorId": "user-123",
            "_links": {
                "webui": "/wiki/spaces/DEV/pages/12345/Architecture+Guide"
            }
        });
        let shaped = shape_page_basic(&raw);
        assert_eq!(shaped["id"], "12345");
        assert_eq!(shaped["title"], "Architecture Guide");
        assert_eq!(shaped["type"], "page");
        assert_eq!(shaped["version"], 5);
    }

    #[test]
    fn shape_search_result_extracts_expected_fields() {
        let raw = json!({
            "id": "12345",
            "title": "API Documentation",
            "type": "page",
            "status": "current",
            "space": {
                "key": "DEV",
                "name": "Development"
            },
            "version": {
                "number": 3,
                "when": "2024-03-01T12:00:00.000Z"
            },
            "history": {
                "createdDate": "2024-01-15T10:00:00.000Z",
                "createdBy": {
                    "accountId": "user-123"
                }
            },
            "excerpt": "This page describes the API endpoints...",
            "_links": {
                "webui": "/wiki/spaces/DEV/pages/12345/API+Documentation"
            }
        });
        let shaped = shape_search_result(&raw);
        assert_eq!(shaped["id"], "12345");
        assert_eq!(shaped["title"], "API Documentation");
        assert_eq!(shaped["type"], "page");
        assert_eq!(shaped["spaceKey"], "DEV");
        assert_eq!(shaped["version"], 3);
        assert_eq!(shaped["createdAt"], "2024-01-15");
        assert_eq!(shaped["lastModified"], "2024-03-01");
        assert_eq!(shaped["authorId"], "user-123");
        assert_eq!(
            shaped["excerpt"],
            "This page describes the API endpoints..."
        );
    }

    #[test]
    fn shape_space_extracts_expected_fields() {
        let raw = json!({
            "id": "123",
            "key": "DEV",
            "name": "Development",
            "type": "global",
            "status": "current",
            "description": {
                "view": {
                    "value": "Development team space"
                }
            },
            "homepageId": "456",
            "_links": {
                "webui": "/wiki/spaces/DEV"
            }
        });
        let shaped = shape_space(&raw);
        assert_eq!(shaped["id"], "123");
        assert_eq!(shaped["key"], "DEV");
        assert_eq!(shaped["name"], "Development");
        assert_eq!(shaped["type"], "global");
    }

    #[test]
    fn date_prefix_normal_date_string() {
        assert_eq!(date_prefix("2024-01-15T10:00:00.000Z"), "2024-01-15");
    }

    #[test]
    fn date_prefix_empty_string() {
        assert_eq!(date_prefix(""), "");
    }

    #[test]
    fn date_prefix_short_string() {
        assert_eq!(date_prefix("2024"), "2024");
    }

    #[test]
    fn date_prefix_exactly_ten_chars() {
        assert_eq!(date_prefix("2024-01-15"), "2024-01-15");
    }

    #[test]
    fn content_detail_level_from_str() {
        assert!(matches!(
            ContentDetailLevel::from_str("basic"),
            ContentDetailLevel::Basic
        ));
        assert!(matches!(
            ContentDetailLevel::from_str("full"),
            ContentDetailLevel::Full
        ));
        assert!(matches!(
            ContentDetailLevel::from_str("standard"),
            ContentDetailLevel::Standard
        ));
        assert!(matches!(
            ContentDetailLevel::from_str("invalid"),
            ContentDetailLevel::Standard
        ));
    }
}
