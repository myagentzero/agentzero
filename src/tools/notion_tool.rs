use super::traits::{Tool, ToolCategory, ToolResult};
use crate::security::{SecurityPolicy, policy::ToolOperation};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const NOTION_API_BASE: &str = "https://api.notion.com/v1";
const NOTION_VERSION: &str = "2022-06-28";
const NOTION_REQUEST_TIMEOUT_SECS: u64 = 30;
/// Retries after a rate-limited (HTTP 429) response before giving up.
const NOTION_RATE_LIMIT_RETRIES: u32 = 3;
/// Upper bound on a single rate-limit wait, regardless of `Retry-After`.
const NOTION_MAX_RETRY_WAIT_SECS: u64 = 10;
/// Notion's maximum (and default) page size for paginated endpoints.
const NOTION_MAX_PAGE_SIZE: u64 = 100;
/// Maximum number of characters to include from an error response body.
const MAX_ERROR_BODY_CHARS: usize = 500;
/// Maximum recursion depth when flattening block children into text. Guards
/// against pathologically deep block trees; if exceeded, traversal stops and
/// reports `truncated: true` rather than silently omitting content.
const MAX_TEXT_TRAVERSAL_DEPTH: usize = 12;
/// Maximum total blocks visited when flattening block children into text.
/// Guards against unbounded traversal cost on very large pages; if exceeded,
/// traversal stops and reports `truncated: true` rather than silently
/// omitting content.
const MAX_TEXT_TRAVERSAL_BLOCKS: usize = 3000;

/// Result of flattening a block subtree into plain text.
struct BlockTextResult {
    text: String,
    truncated: bool,
}

/// Tool for interacting with the Notion API — query databases, read/create/update pages,
/// read (raw or flattened plain text)/append block children (page bodies), and search the
/// workspace. Each action is gated by the appropriate security operation (Read for queries,
/// Act for mutations).
pub struct NotionTool {
    api_key: String,
    http: reqwest::Client,
    security: Arc<SecurityPolicy>,
}

impl NotionTool {
    /// Create a new Notion tool with the given API key and security policy.
    pub fn new(api_key: String, security: Arc<SecurityPolicy>) -> Self {
        Self {
            api_key,
            http: reqwest::Client::new(),
            security,
        }
    }

    /// Build the standard Notion API headers (Authorization, version, content-type).
    fn headers(&self) -> anyhow::Result<reqwest::header::HeaderMap> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.api_key)
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid Notion API key header value: {e}"))?,
        );
        headers.insert("Notion-Version", NOTION_VERSION.parse().unwrap());
        headers.insert("Content-Type", "application/json".parse().unwrap());
        Ok(headers)
    }

    /// Query a Notion database with an optional filter, sorts, pagination, and a
    /// property allowlist (`filter_properties`) to shrink each result.
    async fn query_database(
        &self,
        database_id: &str,
        filter: Option<&serde_json::Value>,
        sorts: Option<&serde_json::Value>,
        start_cursor: Option<&str>,
        page_size: Option<u32>,
        filter_properties: &[String],
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{NOTION_API_BASE}/databases/{database_id}/query");
        let mut body = json!({});
        if let Some(f) = filter {
            body["filter"] = f.clone();
        }
        if let Some(s) = sorts {
            body["sorts"] = s.clone();
        }
        if let Some(cursor) = start_cursor {
            body["start_cursor"] = json!(cursor);
        }
        if let Some(size) = page_size {
            body["page_size"] = json!(size);
        }
        let mut req = self.http.post(&url).headers(self.headers()?).json(&body);
        for prop in filter_properties {
            // Property IDs arrive percent-encoded in responses; decode so the query
            // encoder doesn't double-encode them.
            let decoded =
                urlencoding::decode(prop).map_or_else(|_| prop.clone(), |s| s.into_owned());
            req = req.query(&[("filter_properties", decoded)]);
        }
        let resp = send_with_retry(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated = crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("Notion query_database failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }

    /// Read a single Notion page by ID.
    async fn read_page(&self, page_id: &str) -> anyhow::Result<serde_json::Value> {
        let url = format!("{NOTION_API_BASE}/pages/{page_id}");
        let req = self.http.get(&url).headers(self.headers()?);
        let resp = send_with_retry(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated = crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("Notion read_page failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }

    /// Create a new Notion page, optionally within a database.
    async fn create_page(
        &self,
        properties: &serde_json::Value,
        database_id: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{NOTION_API_BASE}/pages");
        let mut body = json!({ "properties": properties });
        if let Some(db_id) = database_id {
            body["parent"] = json!({ "database_id": db_id });
        }
        let req = self.http.post(&url).headers(self.headers()?).json(&body);
        let resp = send_with_retry(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated = crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("Notion create_page failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }

    /// Update an existing Notion page's properties.
    async fn update_page(
        &self,
        page_id: &str,
        properties: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{NOTION_API_BASE}/pages/{page_id}");
        let body = json!({ "properties": properties });
        let req = self.http.patch(&url).headers(self.headers()?).json(&body);
        let resp = send_with_retry(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated = crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("Notion update_page failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }

    /// Search the Notion workspace by query string, with an optional sort and pagination.
    async fn search(
        &self,
        query: &str,
        sort: Option<&serde_json::Value>,
        start_cursor: Option<&str>,
        page_size: Option<u32>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{NOTION_API_BASE}/search");
        let mut body = json!({ "query": query });
        if let Some(s) = sort {
            body["sort"] = s.clone();
        }
        if let Some(cursor) = start_cursor {
            body["start_cursor"] = json!(cursor);
        }
        if let Some(size) = page_size {
            body["page_size"] = json!(size);
        }
        let req = self.http.post(&url).headers(self.headers()?).json(&body);
        let resp = send_with_retry(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated = crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("Notion search failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }

    /// Read first-level child blocks for a page or block (page body content).
    ///
    /// A page ID is a valid `block_id` for reading that page's body. Nested children
    /// require follow-up calls when a returned block has `has_children: true`.
    async fn read_block_children(
        &self,
        block_id: &str,
        start_cursor: Option<&str>,
        page_size: Option<u32>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{NOTION_API_BASE}/blocks/{block_id}/children");
        let mut req = self.http.get(&url).headers(self.headers()?);
        if let Some(cursor) = start_cursor {
            req = req.query(&[("start_cursor", cursor)]);
        }
        if let Some(size) = page_size {
            req = req.query(&[("page_size", size.to_string())]);
        }
        let resp = send_with_retry(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated = crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("Notion read_block_children failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }

    /// Append child blocks to a page or block body.
    ///
    /// A page ID is a valid `block_id` for writing that page's body. Optionally insert
    /// after a specific sibling via `after`.
    async fn append_blocks(
        &self,
        block_id: &str,
        children: &serde_json::Value,
        after: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{NOTION_API_BASE}/blocks/{block_id}/children");
        let mut body = json!({ "children": children });
        if let Some(after_id) = after {
            body["after"] = json!(after_id);
        }
        let req = self.http.patch(&url).headers(self.headers()?).json(&body);
        let resp = send_with_retry(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let truncated = crate::util::truncate_with_ellipsis(&text, MAX_ERROR_BODY_CHARS);
            anyhow::bail!("Notion append_blocks failed ({status}): {truncated}");
        }
        resp.json().await.map_err(Into::into)
    }

    /// Recursively flatten a block's children into plain text, depth-first, so
    /// meeting-note-style page bodies can be read without walking the raw block
    /// tree by hand. Every visited block either contributes a formatted line or
    /// (for pure layout containers) is transparently recursed into, so content
    /// isn't silently dropped — recognized block types are rendered with
    /// type-appropriate formatting, and any other type falls back to rendering
    /// its `rich_text` (if present) or a `[type]` placeholder, so unfamiliar or
    /// future block types still surface *something* rather than vanishing.
    ///
    /// Traversal is bounded by `MAX_TEXT_TRAVERSAL_DEPTH` and
    /// `MAX_TEXT_TRAVERSAL_BLOCKS` (tracked via `remaining_budget`). Hitting
    /// either bound stops traversal and sets `truncated: true` so callers are
    /// told explicitly instead of receiving quietly incomplete text.
    fn collect_block_text<'a>(
        &'a self,
        block_id: String,
        depth: usize,
        remaining_budget: &'a AtomicUsize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<BlockTextResult>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut text = String::new();
            let mut truncated = false;
            let mut cursor: Option<String> = None;
            let mut numbered_ordinal: usize = 0;

            loop {
                if remaining_budget.load(Ordering::Relaxed) == 0 {
                    truncated = true;
                    break;
                }

                let page = self
                    .read_block_children(&block_id, cursor.as_deref(), Some(100))
                    .await?;
                let results: Vec<serde_json::Value> = page
                    .get("results")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                for block in &results {
                    if remaining_budget.load(Ordering::Relaxed) == 0 {
                        truncated = true;
                        break;
                    }
                    remaining_budget.fetch_sub(1, Ordering::Relaxed);

                    let block_type = block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    numbered_ordinal = if block_type == "numbered_list_item" {
                        numbered_ordinal + 1
                    } else {
                        0
                    };

                    if let Some(line) = render_block_line(block, depth, numbered_ordinal) {
                        text.push_str(&line);
                        text.push('\n');
                    }

                    // Synced blocks that reference another block report their own (empty)
                    // children in the API; the real content lives under `synced_from`.
                    let recurse_id = if block_type == "synced_block" {
                        block
                            .get("synced_block")
                            .and_then(|o| o.get("synced_from"))
                            .and_then(|sf| sf.get("block_id"))
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    } else {
                        None
                    };
                    let has_children = block
                        .get("has_children")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    if has_children || recurse_id.is_some() {
                        let target_id = recurse_id.unwrap_or_else(|| {
                            block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string()
                        });
                        if target_id.is_empty() {
                            continue;
                        }
                        if depth + 1 > MAX_TEXT_TRAVERSAL_DEPTH {
                            text.push_str(&"  ".repeat(depth + 1));
                            text.push_str("[+ nested content not expanded: max depth reached]\n");
                            truncated = true;
                            continue;
                        }
                        let child = self
                            .collect_block_text(target_id, depth + 1, remaining_budget)
                            .await?;
                        text.push_str(&child.text);
                        truncated = truncated || child.truncated;
                    }
                }

                if truncated {
                    break;
                }

                let has_more = page
                    .get("has_more")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !has_more {
                    break;
                }
                cursor = page
                    .get("next_cursor")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if cursor.is_none() {
                    break;
                }
            }

            Ok(BlockTextResult { text, truncated })
        })
    }
}

/// Send a Notion request, waiting and retrying when Notion rate-limits it (HTTP 429).
/// After `NOTION_RATE_LIMIT_RETRIES` waits, the final response is returned as-is.
async fn send_with_retry(req: reqwest::RequestBuilder) -> anyhow::Result<reqwest::Response> {
    let req = req.timeout(std::time::Duration::from_secs(NOTION_REQUEST_TIMEOUT_SECS));
    for _ in 0..NOTION_RATE_LIMIT_RETRIES {
        let Some(attempt) = req.try_clone() else {
            break;
        };
        let resp = attempt.send().await?;
        if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Ok(resp);
        }
        let wait = retry_after_secs(resp.headers())
            .unwrap_or(1)
            .min(NOTION_MAX_RETRY_WAIT_SECS);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    }
    Ok(req.send().await?)
}

/// Parse a `Retry-After` header given in whole seconds (the form Notion sends).
fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Notion IDs are UUIDs: 32 hex digits, with or without dashes. IDs are interpolated
/// into request paths, so anything else (`/`, `..`, `?`) must be rejected.
fn is_valid_notion_id(id: &str) -> bool {
    id.len() <= 36
        && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && id.chars().filter(char::is_ascii_hexdigit).count() == 32
}

/// Concatenate the `plain_text` (falling back to `text.content`) of every rich
/// text run in a Notion rich-text array.
fn extract_rich_text(rich_text: &serde_json::Value) -> String {
    rich_text
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("plain_text")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            item.get("text")
                                .and_then(|t| t.get("content"))
                                .and_then(|v| v.as_str())
                        })
                        .unwrap_or("")
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Render a single Notion block as one formatted, indented line of plain
/// text. Returns `None` only for pure layout containers (`column_list`,
/// `column`, `synced_block`, `table`) whose actual content lives entirely in
/// their children, which the caller recurses into separately.
fn render_block_line(
    block: &serde_json::Value,
    depth: usize,
    numbered_ordinal: usize,
) -> Option<String> {
    let block_type = block
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let type_obj = block.get(block_type);
    let pad = "  ".repeat(depth);
    let rich = |field: &str| -> String {
        type_obj
            .and_then(|o| o.get(field))
            .map(extract_rich_text)
            .unwrap_or_default()
    };

    match block_type {
        "paragraph" => Some(format!("{pad}{}", rich("rich_text"))),
        "heading_1" => Some(format!("{pad}# {}", rich("rich_text"))),
        "heading_2" => Some(format!("{pad}## {}", rich("rich_text"))),
        "heading_3" => Some(format!("{pad}### {}", rich("rich_text"))),
        "bulleted_list_item" => Some(format!("{pad}- {}", rich("rich_text"))),
        "numbered_list_item" => Some(format!("{pad}{numbered_ordinal}. {}", rich("rich_text"))),
        "to_do" => {
            let checked = type_obj
                .and_then(|o| o.get("checked"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let marker = if checked { "[x]" } else { "[ ]" };
            Some(format!("{pad}{marker} {}", rich("rich_text")))
        }
        "quote" => Some(format!("{pad}> {}", rich("rich_text"))),
        "callout" => Some(format!("{pad}> {}", rich("rich_text"))),
        "toggle" => Some(format!("{pad}\u{25b8} {}", rich("rich_text"))),
        "code" => {
            let lang = type_obj
                .and_then(|o| o.get("language"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(format!(
                "{pad}```{lang}\n{pad}{}\n{pad}```",
                rich("rich_text")
            ))
        }
        "equation" => {
            let expr = type_obj
                .and_then(|o| o.get("expression"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(format!("{pad}[equation] {expr}"))
        }
        "divider" => Some(format!("{pad}---")),
        "table_row" => {
            let cells = type_obj
                .and_then(|o| o.get("cells"))
                .and_then(|v| v.as_array());
            let rendered = cells
                .map(|c| {
                    c.iter()
                        .map(extract_rich_text)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default();
            Some(format!("{pad}| {rendered} |"))
        }
        "child_page" => {
            let title = type_obj
                .and_then(|o| o.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("untitled");
            Some(format!("{pad}[child page: {title}]"))
        }
        "child_database" => {
            let title = type_obj
                .and_then(|o| o.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("untitled");
            Some(format!("{pad}[child database: {title}]"))
        }
        "image" | "video" | "file" | "pdf" => {
            let caption = rich("caption");
            let url = type_obj
                .and_then(|o| o.get("external").or_else(|| o.get("file")))
                .and_then(|f| f.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let caption_suffix = if caption.is_empty() {
                String::new()
            } else {
                format!(" {caption}")
            };
            let url_suffix = if url.is_empty() {
                String::new()
            } else {
                format!(" ({url})")
            };
            Some(format!("{pad}[{block_type}]{caption_suffix}{url_suffix}"))
        }
        "bookmark" | "embed" | "link_preview" => {
            let url = type_obj
                .and_then(|o| o.get("url"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(format!("{pad}[{block_type}] {url}"))
        }
        "breadcrumb" | "table_of_contents" => Some(format!("{pad}[{block_type}]")),
        "column_list" | "column" | "synced_block" | "table" => None,
        _ => {
            // Unknown/future block type: surface its rich_text if present so
            // content isn't silently dropped, else a bare placeholder.
            let text = rich("rich_text");
            if text.is_empty() {
                Some(format!("{pad}[{block_type}]"))
            } else {
                Some(format!("{pad}[{block_type}] {text}"))
            }
        }
    }
}

#[async_trait]
impl Tool for NotionTool {
    fn name(&self) -> &str {
        "notion"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::IntegrationTools
    }

    fn description(&self) -> &str {
        "Notion: query databases (filter/sort), read/create/update pages, read (raw or flattened text)/append block children (page bodies), search (optional last_edited_time sort)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "query_database",
                        "read_page",
                        "create_page",
                        "update_page",
                        "read_block_children",
                        "append_blocks",
                        "search"
                    ],
                    "description": "The Notion API action to perform"
                },
                "database_id": {
                    "type": "string",
                    "description": "Database ID (required for query_database, optional for create_page)"
                },
                "page_id": {
                    "type": "string",
                    "description": "Page ID (required for read_page and update_page)"
                },
                "block_id": {
                    "type": "string",
                    "description": "Block or page ID (required for read_block_children and append_blocks). Use a page ID to read/write that page's body."
                },
                "format": {
                    "type": "string",
                    "enum": ["raw", "text"],
                    "description": "For read_block_children: 'text' (default) recursively flattens the entire subtree into plain text (with a truncated flag if a safety limit is hit), more token-efficient than raw JSON; 'raw' returns one page of raw Notion block JSON."
                },
                "filter": {
                    "type": "object",
                    "description": "Notion filter object for query_database"
                },
                "sorts": {
                    "type": "array",
                    "description": "Sort objects for query_database. Each item is a property sort ({property, direction}) or timestamp sort ({timestamp: created_time|last_edited_time, direction}). direction is ascending or descending. Earlier items take precedence."
                },
                "sort": {
                    "type": "object",
                    "description": "Sort object for search. Notion search only supports {timestamp: last_edited_time, direction: ascending|descending}."
                },
                "filter_properties": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "For query_database: return only these properties (IDs or names) in each result. Property IDs appear as properties.<name>.id in results; the title property's ID is 'title'."
                },
                "properties": {
                    "type": "object",
                    "description": "Properties object for create_page and update_page"
                },
                "children": {
                    "type": "array",
                    "description": "Array of Notion block objects for append_blocks"
                },
                "after": {
                    "type": "string",
                    "description": "Optional sibling block ID; append_blocks inserts after this block"
                },
                "start_cursor": {
                    "type": "string",
                    "description": "Pagination cursor (next_cursor from a previous response) for query_database, search, and read_block_children (raw format)"
                },
                "page_size": {
                    "type": "integer",
                    "description": "Max results per call for query_database, search, and read_block_children (raw format). Clamped to 1-100; Notion's default is 100"
                },
                "query": {
                    "type": "string",
                    "description": "Search query string for the search action"
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

        // Enforce granular security: Read for queries, Act for mutations
        let operation = match action {
            "query_database" | "read_page" | "read_block_children" | "search" => {
                ToolOperation::Read
            }
            "create_page" | "update_page" | "append_blocks" => ToolOperation::Act,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Unknown action: {action}. Valid actions: query_database, read_page, create_page, update_page, read_block_children, append_blocks, search"
                    )),
                });
            }
        };

        if let Err(error) = self.security.enforce_tool_operation(operation, "notion") {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        for field in ["database_id", "page_id", "block_id"] {
            if let Some(id) = args.get(field).and_then(|v| v.as_str()) {
                if !is_valid_notion_id(id) {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!(
                            "Invalid {field} '{id}': expected a Notion ID (32 hex characters, dashes optional)"
                        )),
                    });
                }
            }
        }

        let start_cursor = args.get("start_cursor").and_then(|v| v.as_str());
        let page_size = args
            .get("page_size")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, NOTION_MAX_PAGE_SIZE) as u32);

        let result = match action {
            "query_database" => {
                let database_id = match args.get("database_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("query_database requires database_id parameter".into()),
                        });
                    }
                };
                let filter = args.get("filter");
                let sorts = match args.get("sorts") {
                    Some(s) if s.is_array() => Some(s),
                    Some(_) => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("query_database requires sorts to be an array".into()),
                        });
                    }
                    None => None,
                };
                let filter_properties: Vec<String> = match args.get("filter_properties") {
                    None => Vec::new(),
                    Some(v) => match v.as_array().and_then(|items| {
                        items
                            .iter()
                            .map(|item| item.as_str().map(String::from))
                            .collect::<Option<Vec<_>>>()
                    }) {
                        Some(props) => props,
                        None => {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(
                                    "query_database requires filter_properties to be an array of strings"
                                        .into(),
                                ),
                            });
                        }
                    },
                };
                self.query_database(
                    database_id,
                    filter,
                    sorts,
                    start_cursor,
                    page_size,
                    &filter_properties,
                )
                .await
            }
            "read_page" => {
                let page_id = match args.get("page_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("read_page requires page_id parameter".into()),
                        });
                    }
                };
                self.read_page(page_id).await
            }
            "create_page" => {
                let properties = match args.get("properties") {
                    Some(p) => p,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("create_page requires properties parameter".into()),
                        });
                    }
                };
                let database_id = args.get("database_id").and_then(|v| v.as_str());
                self.create_page(properties, database_id).await
            }
            "update_page" => {
                let page_id = match args.get("page_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("update_page requires page_id parameter".into()),
                        });
                    }
                };
                let properties = match args.get("properties") {
                    Some(p) => p,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("update_page requires properties parameter".into()),
                        });
                    }
                };
                self.update_page(page_id, properties).await
            }
            "read_block_children" => {
                let block_id = match args.get("block_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "read_block_children requires block_id parameter (page ID works for page bodies)".into(),
                            ),
                        });
                    }
                };
                let format = args
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("text");
                match format {
                    "raw" => {
                        self.read_block_children(block_id, start_cursor, page_size)
                            .await
                    }
                    "text" => {
                        let budget = AtomicUsize::new(MAX_TEXT_TRAVERSAL_BLOCKS);
                        self.collect_block_text(block_id.to_string(), 0, &budget)
                            .await
                            .map(|result| {
                                json!({
                                    "text": result.text,
                                    "truncated": result.truncated,
                                    "block_count": MAX_TEXT_TRAVERSAL_BLOCKS
                                        - budget.load(Ordering::Relaxed),
                                })
                            })
                    }
                    other => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!(
                                "Invalid format '{other}' for read_block_children. Valid formats: raw, text"
                            )),
                        });
                    }
                }
            }
            "append_blocks" => {
                let block_id = match args.get("block_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(
                                "append_blocks requires block_id parameter (page ID works for page bodies)".into(),
                            ),
                        });
                    }
                };
                let children = match args.get("children") {
                    Some(c) if c.is_array() => c,
                    Some(_) => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("append_blocks requires children to be an array".into()),
                        });
                    }
                    None => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("append_blocks requires children parameter".into()),
                        });
                    }
                };
                let after = args.get("after").and_then(|v| v.as_str());
                self.append_blocks(block_id, children, after).await
            }
            "search" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let sort = match args.get("sort") {
                    Some(s) if s.is_object() => Some(s),
                    Some(_) => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("search requires sort to be an object".into()),
                        });
                    }
                    None => None,
                };
                self.search(query, sort, start_cursor, page_size).await
            }
            _ => unreachable!(), // Already handled above
        };

        match result {
            Ok(value) => Ok(ToolResult {
                success: true,
                output: serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityPolicy;

    const TEST_ID: &str = "00000000-0000-0000-0000-000000000000";

    fn test_tool() -> NotionTool {
        let security = Arc::new(SecurityPolicy::default());
        NotionTool::new("test-key".into(), security)
    }

    #[test]
    fn tool_name_is_notion() {
        let tool = test_tool();
        assert_eq!(tool.name(), "notion");
    }

    #[test]
    fn parameters_schema_has_required_action() {
        let tool = test_tool();
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("action")));
    }

    #[test]
    fn parameters_schema_defines_all_actions() {
        let tool = test_tool();
        let schema = tool.parameters_schema();
        let actions = schema["properties"]["action"]["enum"].as_array().unwrap();
        let action_strs: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
        assert!(action_strs.contains(&"query_database"));
        assert!(action_strs.contains(&"read_page"));
        assert!(action_strs.contains(&"create_page"));
        assert!(action_strs.contains(&"update_page"));
        assert!(action_strs.contains(&"read_block_children"));
        assert!(action_strs.contains(&"append_blocks"));
        assert!(action_strs.contains(&"search"));
    }

    #[tokio::test]
    async fn execute_missing_action_returns_error() {
        let tool = test_tool();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("action"));
    }

    #[tokio::test]
    async fn execute_unknown_action_returns_error() {
        let tool = test_tool();
        let result = tool.execute(json!({"action": "invalid"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("Unknown action"));
    }

    #[test]
    fn parameters_schema_defines_sorts_and_sort() {
        let tool = test_tool();
        let schema = tool.parameters_schema();
        assert_eq!(schema["properties"]["sorts"]["type"], "array");
        assert_eq!(schema["properties"]["sort"]["type"], "object");
    }

    #[test]
    fn parameters_schema_pagination_covers_query_and_search() {
        let tool = test_tool();
        let schema = tool.parameters_schema();
        for field in ["page_size", "start_cursor"] {
            let desc = schema["properties"][field]["description"].as_str().unwrap();
            assert!(desc.contains("query_database"), "{field}");
            assert!(desc.contains("search"), "{field}");
        }
    }

    #[tokio::test]
    async fn execute_query_database_missing_id_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({"action": "query_database"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("database_id"));
    }

    #[tokio::test]
    async fn execute_query_database_sorts_must_be_array() {
        let tool = test_tool();
        let result = tool
            .execute(json!({
                "action": "query_database",
                "database_id": TEST_ID,
                "sorts": {"property": "Name", "direction": "ascending"}
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("sorts"));
    }

    #[test]
    fn is_valid_notion_id_accepts_dashed_and_undashed() {
        assert!(is_valid_notion_id("3b766917-483e-8066-8ed3-e5ce78cf64f9"));
        assert!(is_valid_notion_id("3b766917483e80668ed3e5ce78cf64f9"));
    }

    #[test]
    fn is_valid_notion_id_rejects_path_tricks() {
        for id in [
            "",
            "test-id",
            "../search?x=",
            "3b766917483e80668ed3e5ce78cf64f9/../../search",
            "3b766917483e80668ed3e5ce78cf64f9?x=1",
            "3b766917483e80668ed3e5ce78cf64f",
            "3b766917483e80668ed3e5ce78cf64f9a",
            "--------3b766917483e80668ed3e5ce78cf64f9",
        ] {
            assert!(!is_valid_notion_id(id), "{id}");
        }
    }

    #[tokio::test]
    async fn execute_rejects_invalid_ids_for_each_id_field() {
        let tool = test_tool();
        for (action, field) in [
            ("query_database", "database_id"),
            ("read_page", "page_id"),
            ("read_block_children", "block_id"),
        ] {
            let result = tool
                .execute(json!({"action": action, field: "../search?x="}))
                .await
                .unwrap();
            assert!(!result.success);
            assert!(
                result
                    .error
                    .as_deref()
                    .unwrap()
                    .contains(&format!("Invalid {field}")),
                "{action}"
            );
        }
    }

    #[tokio::test]
    async fn execute_query_database_filter_properties_must_be_string_array() {
        let tool = test_tool();
        for bad in [json!("title"), json!(["title", 1])] {
            let result = tool
                .execute(json!({
                    "action": "query_database",
                    "database_id": TEST_ID,
                    "filter_properties": bad
                }))
                .await
                .unwrap();
            assert!(!result.success);
            assert!(
                result
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("filter_properties")
            );
        }
    }

    #[test]
    fn retry_after_secs_parses_whole_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after_secs(&headers), None);
        headers.insert(reqwest::header::RETRY_AFTER, "2".parse().unwrap());
        assert_eq!(retry_after_secs(&headers), Some(2));
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_secs(&headers), None);
    }

    #[tokio::test]
    async fn execute_search_sort_must_be_object() {
        let tool = test_tool();
        let result = tool
            .execute(json!({
                "action": "search",
                "query": "notes",
                "sort": "last_edited_time"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("sort"));
    }

    #[tokio::test]
    async fn execute_read_page_missing_id_returns_error() {
        let tool = test_tool();
        let result = tool.execute(json!({"action": "read_page"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("page_id"));
    }

    #[tokio::test]
    async fn execute_create_page_missing_properties_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({"action": "create_page"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("properties"));
    }

    #[tokio::test]
    async fn execute_update_page_missing_page_id_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({"action": "update_page", "properties": {}}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("page_id"));
    }

    #[tokio::test]
    async fn execute_update_page_missing_properties_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({"action": "update_page", "page_id": TEST_ID}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("properties"));
    }

    #[tokio::test]
    async fn execute_read_block_children_missing_block_id_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({"action": "read_block_children"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("block_id"));
    }

    #[tokio::test]
    async fn execute_append_blocks_missing_block_id_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({
                "action": "append_blocks",
                "children": [{"type": "paragraph", "paragraph": {"rich_text": []}}]
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("block_id"));
    }

    #[tokio::test]
    async fn execute_append_blocks_missing_children_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({"action": "append_blocks", "block_id": TEST_ID}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("children"));
    }

    #[tokio::test]
    async fn execute_append_blocks_children_must_be_array() {
        let tool = test_tool();
        let result = tool
            .execute(json!({
                "action": "append_blocks",
                "block_id": TEST_ID,
                "children": {"type": "paragraph"}
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("array"));
    }

    #[tokio::test]
    async fn execute_read_block_children_invalid_format_returns_error() {
        let tool = test_tool();
        let result = tool
            .execute(json!({
                "action": "read_block_children",
                "block_id": TEST_ID,
                "format": "yaml"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap().contains("Invalid format"));
    }

    #[test]
    fn extract_rich_text_joins_plain_text_runs() {
        let rich_text = json!([
            {"plain_text": "Hello, "},
            {"plain_text": "world!"}
        ]);
        assert_eq!(extract_rich_text(&rich_text), "Hello, world!");
    }

    #[test]
    fn extract_rich_text_falls_back_to_text_content() {
        let rich_text = json!([{"type": "text", "text": {"content": "fallback"}}]);
        assert_eq!(extract_rich_text(&rich_text), "fallback");
    }

    #[test]
    fn extract_rich_text_handles_empty_array() {
        assert_eq!(extract_rich_text(&json!([])), "");
    }

    #[test]
    fn render_block_line_paragraph() {
        let block = json!({
            "type": "paragraph",
            "paragraph": {"rich_text": [{"plain_text": "Discuss roadmap"}]}
        });
        assert_eq!(
            render_block_line(&block, 0, 0),
            Some("Discuss roadmap".to_string())
        );
    }

    #[test]
    fn render_block_line_heading_and_indentation() {
        let block = json!({
            "type": "heading_2",
            "heading_2": {"rich_text": [{"plain_text": "Action Items"}]}
        });
        assert_eq!(
            render_block_line(&block, 1, 0),
            Some("  ## Action Items".to_string())
        );
    }

    #[test]
    fn render_block_line_numbered_list_uses_ordinal() {
        let block = json!({
            "type": "numbered_list_item",
            "numbered_list_item": {"rich_text": [{"plain_text": "First step"}]}
        });
        assert_eq!(
            render_block_line(&block, 0, 3),
            Some("3. First step".to_string())
        );
    }

    #[test]
    fn render_block_line_to_do_reflects_checked_state() {
        let checked = json!({
            "type": "to_do",
            "to_do": {"rich_text": [{"plain_text": "Send notes"}], "checked": true}
        });
        assert_eq!(
            render_block_line(&checked, 0, 0),
            Some("[x] Send notes".to_string())
        );

        let unchecked = json!({
            "type": "to_do",
            "to_do": {"rich_text": [{"plain_text": "Send notes"}], "checked": false}
        });
        assert_eq!(
            render_block_line(&unchecked, 0, 0),
            Some("[ ] Send notes".to_string())
        );
    }

    #[test]
    fn render_block_line_layout_containers_return_none() {
        for ty in ["column_list", "column", "synced_block", "table"] {
            let block = json!({"type": ty});
            assert_eq!(render_block_line(&block, 0, 0), None, "type: {ty}");
        }
    }

    #[test]
    fn render_block_line_unknown_type_with_rich_text_is_not_lost() {
        let block = json!({
            "type": "some_future_block_type",
            "some_future_block_type": {"rich_text": [{"plain_text": "future content"}]}
        });
        assert_eq!(
            render_block_line(&block, 0, 0),
            Some("[some_future_block_type] future content".to_string())
        );
    }

    #[test]
    fn render_block_line_unknown_type_without_rich_text_gets_placeholder() {
        let block = json!({"type": "some_unhandled_type"});
        assert_eq!(
            render_block_line(&block, 0, 0),
            Some("[some_unhandled_type]".to_string())
        );
    }
}
