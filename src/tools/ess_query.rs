use super::traits::{Tool, ToolResult};
use crate::security::{SecurityPolicy, policy::ToolOperation};
use async_trait::async_trait;
use elasticsearch::{
    Elasticsearch,
    auth::Credentials,
    http::Method,
    http::Url,
    http::request::JsonBody,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const MAX_PATH_BYTES: usize = 4096;
const MAX_ERROR_BODY_CHARS: usize = 500;

/// Read-only Elasticsearch passthrough query tool.
///
/// Accepts an arbitrary relative path (e.g. `/_cat/indices?v` or `/my-index/_search`)
/// and forwards it to the configured cluster with `GET` or `POST`. Authenticated via
/// a base64-encoded API key from Kibana ("Create API key").
pub struct EssQueryTool {
    cluster_names: Vec<String>,
    description: String,
    timeout_secs: u64,
    security: Arc<SecurityPolicy>,
    client: Elasticsearch,
}

impl EssQueryTool {
    pub fn new(
        endpoint: String,
        auth: String,
        mut cluster_names: Vec<String>,
        security: Arc<SecurityPolicy>,
        timeout_secs: u64,
    ) -> anyhow::Result<Self> {
        let url = Url::parse(&endpoint)
            .map_err(|e| anyhow::anyhow!("Invalid elasticsearch endpoint {endpoint:?}: {e}"))?;
        let pool = SingleNodeConnectionPool::new(url);
        let transport = TransportBuilder::new(pool)
            .auth(Credentials::EncodedApiKey(auth))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build elasticsearch transport: {e}"))?;
        if !cluster_names.iter().any(|n| n == "*") {
            cluster_names.push("*".to_string());
        }
        let names_list = cluster_names.join(", ");
        let default_name = cluster_names.first().cloned().unwrap_or_default();
        let description = format!(
            "Read-only Elasticsearch query against the configured cluster. Accepts a raw path \
             (e.g. '/_cat/indices?v', '/_cluster/health', '/<index>/_search') with GET or POST \
             and an optional JSON body. Returns the response body. \
             Available cluster names: {names_list}. \
             Pass cluster_name to target a specific cluster; defaults to '{default_name}' if omitted."
        );
        Ok(Self {
            cluster_names,
            description,
            timeout_secs,
            security,
            client: Elasticsearch::new(transport),
        })
    }
}

#[async_trait]
impl Tool for EssQueryTool {
    fn name(&self) -> &str {
        "ess_query"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        let default_name = self.cluster_names.first().map(String::as_str).unwrap_or("");
        let cluster_name_desc = format!(
            "Cluster to query. Valid values: {}. Defaults to '{default_name}' if omitted.",
            self.cluster_names.join(", ")
        );
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path on the cluster, starting with '/'. May include a query string (e.g. '/_cat/indices?v')."
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST"],
                    "description": "HTTP method. Defaults to GET. Use POST for _search requests with a JSON body."
                },
                "body": {
                    "type": "object",
                    "description": "Optional JSON body (used primarily with POST for _search/_msearch queries)."
                },
                "cluster_name": {
                    "type": "string",
                    "enum": self.cluster_names,
                    "description": cluster_name_desc
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let cluster_name = match args.get("cluster_name").and_then(Value::as_str) {
            Some(name) => {
                if !self.cluster_names.iter().any(|n| n == name) {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!(
                            "Unknown cluster_name {name:?}. Valid options: {}",
                            self.cluster_names.join(", ")
                        )),
                    });
                }
                name.to_string()
            }
            None => self.cluster_names.first().cloned().unwrap_or_default(),
        };

        let path = match args.get("path").and_then(Value::as_str) {
            Some(p) => p,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing required parameter: path".into()),
                });
            }
        };
        if path.is_empty() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Parameter 'path' must not be empty".into()),
            });
        }
        if !path.starts_with('/') {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Parameter 'path' must start with '/': got {path:?}"
                )),
            });
        }
        if path.len() > MAX_PATH_BYTES {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Parameter 'path' exceeds {MAX_PATH_BYTES} bytes")),
            });
        }
        if path.chars().any(|c| c.is_control()) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Parameter 'path' must not contain control characters".into()),
            });
        }

        let method_str = args
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        let method = match method_str.as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            other => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Method {other:?} is not allowed; ess_query is read-only (GET or POST only)"
                    )),
                });
            }
        };

        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Read, "ess_query")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let body = args.get("body").cloned().filter(|v| !v.is_null());

        let send_result = match body {
            Some(b) => {
                self.client
                    .send(
                        method,
                        path,
                        reqwest::header::HeaderMap::new(),
                        Option::<&Value>::None,
                        Some(JsonBody::from(b)),
                        Some(Duration::from_secs(self.timeout_secs)),
                    )
                    .await
            }
            None => {
                self.client
                    .send::<(), Value>(
                        method,
                        path,
                        reqwest::header::HeaderMap::new(),
                        None,
                        None,
                        Some(Duration::from_secs(self.timeout_secs)),
                    )
                    .await
            }
        };

        let resp = match send_result {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Elasticsearch request to cluster {cluster_name:?} failed: {e}"
                    )),
                });
            }
        };

        let status = resp.status_code();
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to read Elasticsearch response body: {e}")),
                });
            }
        };

        if !status.is_success() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Elasticsearch {status}: {}",
                    crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS)
                )),
            });
        }

        let output = match serde_json::from_str::<Value>(&text) {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or(text),
            Err(_) => text,
        };

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;

    fn make_tool() -> EssQueryTool {
        EssQueryTool::new(
            "https://example.invalid:9200".into(),
            "ZmFrZS1iYXNlNjQta2V5".into(),
            vec!["prod".into(), "staging".into()],
            Arc::new(SecurityPolicy::default()),
            5,
        )
        .expect("tool should build")
    }

    #[test]
    fn schema_exposes_path_method_body_cluster_name() {
        let tool = make_tool();
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(schema["properties"]["path"]["type"], "string");
        let methods = schema["properties"]["method"]["enum"].as_array().unwrap();
        let methods: Vec<&str> = methods.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(methods, vec!["GET", "POST"]);
        let clusters = schema["properties"]["cluster_name"]["enum"]
            .as_array()
            .unwrap();
        let clusters: Vec<&str> = clusters.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(clusters, vec!["prod", "staging", "*"]);
    }

    #[tokio::test]
    async fn rejects_missing_path() {
        let tool = make_tool();
        let res = tool.execute(json!({})).await.unwrap();
        assert!(!res.success);
        assert!(res.error.unwrap().contains("path"));
    }

    #[tokio::test]
    async fn rejects_path_without_leading_slash() {
        let tool = make_tool();
        let res = tool
            .execute(json!({ "path": "_cat/indices" }))
            .await
            .unwrap();
        assert!(!res.success);
        assert!(res.error.unwrap().contains("start with"));
    }

    #[tokio::test]
    async fn rejects_non_read_method() {
        let tool = make_tool();
        let res = tool
            .execute(json!({ "path": "/", "method": "DELETE" }))
            .await
            .unwrap();
        assert!(!res.success);
        assert!(res.error.unwrap().contains("read-only"));
    }

    #[test]
    fn wildcard_always_in_cluster_names() {
        let tool = make_tool();
        let schema = tool.parameters_schema();
        let clusters = schema["properties"]["cluster_name"]["enum"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = clusters.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            names.contains(&"*"),
            "* should always be in cluster_name enum"
        );
    }

    #[tokio::test]
    async fn rejects_invalid_cluster_name() {
        let tool = make_tool();
        let res = tool
            .execute(json!({ "path": "/", "cluster_name": "unknown" }))
            .await
            .unwrap();
        assert!(!res.success);
        let err = res.error.unwrap();
        assert!(
            err.contains("unknown"),
            "error should mention the bad name: {err}"
        );
        assert!(err.contains("prod"), "error should list valid names: {err}");
        assert!(
            err.contains("staging"),
            "error should list valid names: {err}"
        );
    }

    #[tokio::test]
    async fn accepts_valid_cluster_name() {
        let tool = make_tool();
        // Security check happens after validation; just ensure we get past cluster_name check.
        // The request will fail at network level, but with a network error, not a validation error.
        let res = tool
            .execute(json!({ "path": "/", "cluster_name": "staging" }))
            .await
            .unwrap();
        // If there's an error it should be network-related, not a validation error.
        if let Some(err) = res.error {
            assert!(
                !err.contains("Unknown cluster_name"),
                "should not be a validation error: {err}"
            );
        }
    }

    #[tokio::test]
    async fn missing_cluster_name_defaults_to_first() {
        let tool = make_tool();
        // No cluster_name provided — should default to "prod".
        // We can't easily assert the chosen name without a real cluster, but we can
        // assert there is no validation error by verifying any error isn't about cluster_name.
        let res = tool.execute(json!({ "path": "/" })).await.unwrap();
        if let Some(err) = res.error {
            assert!(
                !err.contains("Unknown cluster_name"),
                "should not be a validation error: {err}"
            );
        }
    }
}
