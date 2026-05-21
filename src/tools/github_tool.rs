use super::traits::{Tool, ToolResult};
use crate::config::schema::DEFAULT_USER_AGENT;
use crate::security::{SecurityPolicy, policy::ToolOperation};
use async_trait::async_trait;
use reqwest::{Client, Method, RequestBuilder, StatusCode, header::HeaderMap};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const MAX_ERROR_BODY_CHARS: usize = 500;
const MAX_RETRIES: u32 = 3;

/// Tool for interacting with the GitHub REST API.
///
/// Actions are gated by `[github].allowed_actions` and the security policy's
/// Read/Act split. Repos are restricted by `[github].allowed_repos`, which
/// supports `*`, `owner/*`, and `owner/repo` wildcards.
///
/// `create_pull_request` requires the `head` branch to already exist on remote;
/// the tool does not push code. `merge_pr` is destructive against the base
/// branch — leave it out of `allowed_actions` unless explicitly needed.
/// Tool calls are not idempotent: a retried call can double-comment or
/// double-open a PR.
pub struct GitHubTool {
    access_token: String,
    api_base_url: String,
    allowed_repos: Vec<String>,
    allowed_actions: Vec<String>,
    http: Client,
    security: Arc<SecurityPolicy>,
    timeout_secs: u64,
}

impl GitHubTool {
    pub fn new(
        access_token: String,
        api_base_url: Option<String>,
        allowed_repos: Vec<String>,
        allowed_actions: Vec<String>,
        security: Arc<SecurityPolicy>,
        timeout_secs: u64,
    ) -> Self {
        let base = api_base_url
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(DEFAULT_GITHUB_API_BASE);
        Self {
            access_token,
            api_base_url: base.trim_end_matches('/').to_string(),
            allowed_repos,
            allowed_actions,
            http: Client::new(),
            security,
            timeout_secs,
        }
    }

    fn is_action_allowed(&self, action: &str) -> bool {
        self.allowed_actions.iter().any(|a| a == action)
    }

    fn repo_is_allowed(&self, repo_full_name: &str) -> bool {
        if self.allowed_repos.is_empty() {
            return false;
        }
        self.allowed_repos.iter().any(|raw| {
            let allowed = raw.trim();
            if allowed.is_empty() {
                return false;
            }
            if allowed == "*" {
                return true;
            }
            if let Some(owner_prefix) = allowed.strip_suffix("/*") {
                if let Some((repo_owner, _)) = repo_full_name.split_once('/') {
                    return repo_owner.eq_ignore_ascii_case(owner_prefix);
                }
            }
            repo_full_name.eq_ignore_ascii_case(allowed)
        })
    }

    fn now_unix_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn is_rate_limited(status: StatusCode, headers: &HeaderMap) -> bool {
        if status == StatusCode::TOO_MANY_REQUESTS {
            return true;
        }
        status == StatusCode::FORBIDDEN
            && headers
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .is_some_and(|v| v == "0")
    }

    fn retry_delay_from_headers(headers: &HeaderMap) -> Option<Duration> {
        if let Some(raw) = headers.get("retry-after").and_then(|v| v.to_str().ok()) {
            if let Ok(secs) = raw.trim().parse::<u64>() {
                return Some(Duration::from_secs(secs.max(1).min(60)));
            }
        }

        let remaining_is_zero = headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .is_some_and(|v| v == "0");
        if !remaining_is_zero {
            return None;
        }

        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())?;
        let now = Self::now_unix_secs();
        let wait = if reset > now { reset - now } else { 1 };
        Some(Duration::from_secs(wait.max(1).min(60)))
    }

    fn build_request(&self, method: Method, url: &str) -> RequestBuilder {
        self.http
            .request(method, url)
            .bearer_auth(&self.access_token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header("User-Agent", DEFAULT_USER_AGENT)
            .timeout(Duration::from_secs(self.timeout_secs))
    }

    /// Send a request with up to 3 attempts, honoring GitHub rate-limit headers.
    /// `body` is captured so we can rebuild the request on retry.
    async fn send_with_retry(
        &self,
        method: Method,
        url: &str,
        body: Option<Value>,
        action: &str,
    ) -> anyhow::Result<Value> {
        let mut backoff = Duration::from_secs(1);

        for attempt in 1..=MAX_RETRIES {
            let mut req = self.build_request(method.clone(), url);
            if let Some(payload) = &body {
                req = req.json(payload);
            }

            let response = req
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("GitHub {action} request failed: {e}"))?;

            let status = response.status();
            if status.is_success() {
                let body_text = response.text().await.unwrap_or_default();
                if body_text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&body_text)
                    .map_err(|e| anyhow::anyhow!("GitHub {action} returned invalid JSON: {e}"));
            }

            let headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            let sanitized = crate::util::truncate_with_ellipsis(
                &crate::providers::sanitize_api_error(&body_text),
                MAX_ERROR_BODY_CHARS,
            );

            if attempt < MAX_RETRIES && Self::is_rate_limited(status, &headers) {
                let wait = Self::retry_delay_from_headers(&headers).unwrap_or(backoff);
                tracing::warn!(
                    "GitHub {action} rate-limited (status {status}), retrying in {}s (attempt {attempt}/{MAX_RETRIES})",
                    wait.as_secs()
                );
                tokio::time::sleep(wait).await;
                backoff = (backoff * 2).min(Duration::from_secs(8));
                continue;
            }

            anyhow::bail!("GitHub {action} failed ({status}): {sanitized}");
        }

        anyhow::bail!("GitHub {action} retries exhausted")
    }

    fn check_repo(&self, repo: &str) -> anyhow::Result<()> {
        validate_repo(repo)?;
        if !self.repo_is_allowed(repo) {
            anyhow::bail!("Repo '{repo}' is not in allowed_repos. Configure github.allowed_repos.");
        }
        Ok(())
    }

    fn repo_url(&self, repo: &str, suffix: &str) -> String {
        let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
        let owner = urlencoding::encode(owner.trim());
        let name = urlencoding::encode(name.trim());
        format!("{}/repos/{owner}/{name}{suffix}", self.api_base_url)
    }

    async fn get_issue(&self, repo: &str, issue_number: u64) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        let url = self.repo_url(repo, &format!("/issues/{issue_number}"));
        let raw = self
            .send_with_retry(Method::GET, &url, None, "get_issue")
            .await?;
        let shaped = shape_issue(&raw);
        Ok(ok_json(shaped))
    }

    async fn get_pr(&self, repo: &str, pr_number: u64) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        let url = self.repo_url(repo, &format!("/pulls/{pr_number}"));
        let raw = self
            .send_with_retry(Method::GET, &url, None, "get_pr")
            .await?;
        let shaped = shape_pr(&raw);
        Ok(ok_json(shaped))
    }

    async fn list_comments(&self, repo: &str, issue_number: u64) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        let url = self.repo_url(repo, &format!("/issues/{issue_number}/comments"));
        let raw = self
            .send_with_retry(Method::GET, &url, None, "list_comments")
            .await?;
        let shaped = shape_comments(&raw);
        Ok(ok_json(shaped))
    }

    async fn list_pr_review_comments(
        &self,
        repo: &str,
        pr_number: u64,
    ) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        let url = self.repo_url(repo, &format!("/pulls/{pr_number}/comments"));
        let raw = self
            .send_with_retry(Method::GET, &url, None, "list_pr_review_comments")
            .await?;
        let shaped = shape_review_comments(&raw);
        Ok(ok_json(shaped))
    }

    async fn add_comment(
        &self,
        repo: &str,
        issue_number: u64,
        body: &str,
    ) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        let url = self.repo_url(repo, &format!("/issues/{issue_number}/comments"));
        let payload = json!({ "body": body });
        let raw = self
            .send_with_retry(Method::POST, &url, Some(payload), "add_comment")
            .await?;
        let shaped = json!({
            "id": raw["id"],
            "html_url": raw["html_url"],
            "created_at": raw["created_at"],
        });
        Ok(ok_json(shaped))
    }

    async fn create_pull_request(
        &self,
        repo: &str,
        title: &str,
        head: &str,
        base: &str,
        body: Option<&str>,
        draft: Option<bool>,
    ) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        validate_branch_ref(head, "head")?;
        validate_branch_ref(base, "base")?;
        if head == base {
            anyhow::bail!("create_pull_request: head and base must differ");
        }

        let url = self.repo_url(repo, "/pulls");
        let mut payload = json!({
            "title": title,
            "head": head,
            "base": base,
        });
        if let Some(b) = body {
            payload["body"] = json!(b);
        }
        if let Some(d) = draft {
            payload["draft"] = json!(d);
        }

        let raw = self
            .send_with_retry(Method::POST, &url, Some(payload), "create_pull_request")
            .await?;
        Ok(ok_json(shape_pr(&raw)))
    }

    async fn close_pr(&self, repo: &str, pr_number: u64) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        let url = self.repo_url(repo, &format!("/pulls/{pr_number}"));
        let payload = json!({ "state": "closed" });
        let raw = self
            .send_with_retry(Method::PATCH, &url, Some(payload), "close_pr")
            .await?;
        Ok(ok_json(shape_pr(&raw)))
    }

    async fn merge_pr(
        &self,
        repo: &str,
        pr_number: u64,
        merge_method: Option<&str>,
    ) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        let method = merge_method.unwrap_or("merge");
        if !matches!(method, "merge" | "squash" | "rebase") {
            anyhow::bail!("merge_pr: merge_method must be one of merge, squash, rebase");
        }
        let url = self.repo_url(repo, &format!("/pulls/{pr_number}/merge"));
        let payload = json!({ "merge_method": method });
        let raw = self
            .send_with_retry(Method::PUT, &url, Some(payload), "merge_pr")
            .await?;
        let shaped = json!({
            "merged": raw["merged"],
            "sha": raw["sha"],
            "message": raw["message"],
        });
        Ok(ok_json(shaped))
    }

    async fn request_review(
        &self,
        repo: &str,
        pr_number: u64,
        reviewers: Vec<String>,
    ) -> anyhow::Result<ToolResult> {
        self.check_repo(repo)?;
        if reviewers.is_empty() {
            anyhow::bail!("request_review: reviewers must be a non-empty array");
        }
        let url = self.repo_url(repo, &format!("/pulls/{pr_number}/requested_reviewers"));
        let payload = json!({ "reviewers": reviewers });
        let raw = self
            .send_with_retry(Method::POST, &url, Some(payload), "request_review")
            .await?;
        let shaped = json!({
            "number": raw["number"],
            "html_url": raw["html_url"],
            "requested_reviewers": raw["requested_reviewers"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|r| r["login"].as_str()).collect::<Vec<_>>())
                .unwrap_or_default(),
        });
        Ok(ok_json(shaped))
    }
}

#[async_trait]
impl Tool for GitHubTool {
    fn name(&self) -> &str {
        "github"
    }

    fn description(&self) -> &str {
        "Interact with GitHub issues and PRs: read details, list comments, add comments, create/close/merge PRs, request reviews. Use when: user references a GitHub issue/PR, asks about its state, or wants to comment, open, close, merge, or request reviewers. Don't use when: discussing GitHub conceptually without needing live data. Note: create_pull_request requires the head branch to already exist on remote, the tool does not push code (use git_operations)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "get_issue",
                        "get_pr",
                        "list_comments",
                        "list_pr_review_comments",
                        "add_comment",
                        "create_pull_request",
                        "close_pr",
                        "merge_pr",
                        "request_review"
                    ],
                    "description": "GitHub action to perform. Enabled actions are configured in [github].allowed_actions."
                },
                "repo": {
                    "type": "string",
                    "description": "Repository in 'owner/repo' format. Required for all actions. Must match github.allowed_repos."
                },
                "issue_number": {
                    "type": "integer",
                    "description": "Issue number for get_issue, list_comments, add_comment."
                },
                "pr_number": {
                    "type": "integer",
                    "description": "Pull request number for get_pr, list_pr_review_comments, close_pr, merge_pr, request_review."
                },
                "body": {
                    "type": "string",
                    "description": "Comment body for add_comment, or PR description for create_pull_request."
                },
                "title": {
                    "type": "string",
                    "description": "PR title for create_pull_request."
                },
                "head": {
                    "type": "string",
                    "description": "Source branch for create_pull_request. Must exist on remote (this tool does not push)."
                },
                "base": {
                    "type": "string",
                    "description": "Target branch for create_pull_request."
                },
                "draft": {
                    "type": "boolean",
                    "description": "Whether to open the PR as a draft (create_pull_request)."
                },
                "merge_method": {
                    "type": "string",
                    "enum": ["merge", "squash", "rebase"],
                    "description": "Merge strategy for merge_pr. Defaults to 'merge'."
                },
                "reviewers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of GitHub usernames to request review from (request_review)."
                }
            },
            "required": ["action", "repo"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Ok(missing("action")),
        };

        if !matches!(
            action,
            "get_issue"
                | "get_pr"
                | "list_comments"
                | "list_pr_review_comments"
                | "add_comment"
                | "create_pull_request"
                | "close_pr"
                | "merge_pr"
                | "request_review"
        ) {
            return Ok(err(format!(
                "Unknown action: '{action}'. Valid actions: get_issue, get_pr, list_comments, list_pr_review_comments, add_comment, create_pull_request, close_pr, merge_pr, request_review"
            )));
        }

        if !self.is_action_allowed(action) {
            return Ok(err(format!(
                "Action '{action}' is not enabled. Add it to github.allowed_actions in config.toml. \
                 Currently allowed: {}",
                self.allowed_actions.join(", ")
            )));
        }

        let operation = match action {
            "get_issue" | "get_pr" | "list_comments" | "list_pr_review_comments" => {
                ToolOperation::Read
            }
            "add_comment" | "create_pull_request" | "close_pr" | "merge_pr" | "request_review" => {
                ToolOperation::Act
            }
            _ => unreachable!(),
        };

        if let Err(error) = self.security.enforce_tool_operation(operation, "github") {
            return Ok(err(error));
        }

        let repo = match args.get("repo").and_then(|v| v.as_str()) {
            Some(r) => r,
            None => return Ok(missing("repo")),
        };

        let result = match action {
            "get_issue" => match args.get("issue_number").and_then(Value::as_u64) {
                Some(n) if n > 0 => self.get_issue(repo, n).await,
                _ => return Ok(missing("issue_number")),
            },
            "get_pr" => match args.get("pr_number").and_then(Value::as_u64) {
                Some(n) if n > 0 => self.get_pr(repo, n).await,
                _ => return Ok(missing("pr_number")),
            },
            "list_comments" => match args.get("issue_number").and_then(Value::as_u64) {
                Some(n) if n > 0 => self.list_comments(repo, n).await,
                _ => return Ok(missing("issue_number")),
            },
            "list_pr_review_comments" => match args.get("pr_number").and_then(Value::as_u64) {
                Some(n) if n > 0 => self.list_pr_review_comments(repo, n).await,
                _ => return Ok(missing("pr_number")),
            },
            "add_comment" => {
                let issue_number = match args.get("issue_number").and_then(Value::as_u64) {
                    Some(n) if n > 0 => n,
                    _ => return Ok(missing("issue_number")),
                };
                let body = match args.get("body").and_then(|v| v.as_str()) {
                    Some(b) if !b.trim().is_empty() => b,
                    _ => return Ok(missing("body")),
                };
                self.add_comment(repo, issue_number, body).await
            }
            "create_pull_request" => {
                let title = match args.get("title").and_then(|v| v.as_str()) {
                    Some(t) if !t.trim().is_empty() => t,
                    _ => return Ok(missing("title")),
                };
                let head = match args.get("head").and_then(|v| v.as_str()) {
                    Some(h) if !h.trim().is_empty() => h,
                    _ => return Ok(missing("head")),
                };
                let base = match args.get("base").and_then(|v| v.as_str()) {
                    Some(b) if !b.trim().is_empty() => b,
                    _ => return Ok(missing("base")),
                };
                let body = args.get("body").and_then(|v| v.as_str());
                let draft = args.get("draft").and_then(Value::as_bool);
                self.create_pull_request(repo, title, head, base, body, draft)
                    .await
            }
            "close_pr" => match args.get("pr_number").and_then(Value::as_u64) {
                Some(n) if n > 0 => self.close_pr(repo, n).await,
                _ => return Ok(missing("pr_number")),
            },
            "merge_pr" => {
                let pr_number = match args.get("pr_number").and_then(Value::as_u64) {
                    Some(n) if n > 0 => n,
                    _ => return Ok(missing("pr_number")),
                };
                let merge_method = args.get("merge_method").and_then(|v| v.as_str());
                self.merge_pr(repo, pr_number, merge_method).await
            }
            "request_review" => {
                let pr_number = match args.get("pr_number").and_then(Value::as_u64) {
                    Some(n) if n > 0 => n,
                    _ => return Ok(missing("pr_number")),
                };
                let reviewers: Vec<String> = args
                    .get("reviewers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if reviewers.is_empty() {
                    return Ok(missing("reviewers"));
                }
                self.request_review(repo, pr_number, reviewers).await
            }
            _ => unreachable!(),
        };

        match result {
            Ok(tr) => Ok(tr),
            Err(e) => Ok(err(e.to_string())),
        }
    }
}

// ── Validation ──────────────────────────────────────────────────────────

fn validate_repo(repo: &str) -> anyhow::Result<()> {
    let trimmed = repo.trim();
    if trimmed.contains("..") || trimmed.contains(char::is_whitespace) {
        anyhow::bail!("Invalid repo '{repo}': must not contain '..' or whitespace");
    }
    let Some((owner, name)) = trimmed.split_once('/') else {
        anyhow::bail!("Invalid repo '{repo}': expected 'owner/repo' format");
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        anyhow::bail!("Invalid repo '{repo}': expected 'owner/repo' format");
    }
    let valid = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !valid(owner) || !valid(name) {
        anyhow::bail!("Invalid repo '{repo}': only ASCII alphanumeric, '-', '_', '.' allowed");
    }
    Ok(())
}

fn validate_branch_ref(value: &str, label: &str) -> anyhow::Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{label} must be non-empty");
    }
    if trimmed.contains("..") || trimmed.contains(char::is_whitespace) {
        anyhow::bail!("{label} '{value}' must not contain '..' or whitespace");
    }
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn ok_json(v: Value) -> ToolResult {
    ToolResult {
        success: true,
        output: serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
        error: None,
    }
}

fn err(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        success: false,
        output: String::new(),
        error: Some(msg.into()),
    }
}

fn missing(field: &str) -> ToolResult {
    err(format!("Missing required parameter: {field}"))
}

// ── Response shaping ────────────────────────────────────────────────────

fn shape_issue(raw: &Value) -> Value {
    json!({
        "number": raw["number"],
        "title": raw["title"],
        "state": raw["state"],
        "body": raw["body"],
        "labels": raw["labels"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|l| l["name"].as_str()).collect::<Vec<_>>())
            .unwrap_or_default(),
        "author": raw["user"]["login"],
        "created_at": raw["created_at"],
        "updated_at": raw["updated_at"],
        "html_url": raw["html_url"],
        "comments_count": raw["comments"],
    })
}

fn shape_pr(raw: &Value) -> Value {
    json!({
        "number": raw["number"],
        "title": raw["title"],
        "state": raw["state"],
        "draft": raw["draft"],
        "merged": raw["merged"],
        "body": raw["body"],
        "head": raw["head"]["ref"],
        "base": raw["base"]["ref"],
        "author": raw["user"]["login"],
        "created_at": raw["created_at"],
        "updated_at": raw["updated_at"],
        "html_url": raw["html_url"],
    })
}

fn shape_comments(raw: &Value) -> Value {
    let arr = raw.as_array().cloned().unwrap_or_default();
    let shaped: Vec<Value> = arr
        .iter()
        .map(|c| {
            json!({
                "id": c["id"],
                "author": c["user"]["login"],
                "body": c["body"],
                "created_at": c["created_at"],
                "html_url": c["html_url"],
            })
        })
        .collect();
    json!(shaped)
}

fn shape_review_comments(raw: &Value) -> Value {
    let arr = raw.as_array().cloned().unwrap_or_default();
    let shaped: Vec<Value> = arr
        .iter()
        .map(|c| {
            json!({
                "id": c["id"],
                "author": c["user"]["login"],
                "body": c["body"],
                "path": c["path"],
                "created_at": c["created_at"],
                "html_url": c["html_url"],
            })
        })
        .collect();
    json!(shaped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::AutonomyLevel;

    fn make_tool(allowed_actions: Vec<&str>, allowed_repos: Vec<&str>) -> GitHubTool {
        let security = Arc::new(SecurityPolicy::default());
        GitHubTool::new(
            "ghp_test".into(),
            None,
            allowed_repos.into_iter().map(String::from).collect(),
            allowed_actions.into_iter().map(String::from).collect(),
            security,
            30,
        )
    }

    fn make_tool_with_autonomy(allowed_actions: Vec<&str>, autonomy: AutonomyLevel) -> GitHubTool {
        let security = Arc::new(SecurityPolicy {
            autonomy,
            ..SecurityPolicy::default()
        });
        GitHubTool::new(
            "ghp_test".into(),
            None,
            vec!["myagentzero/zeroclaw".to_string()],
            allowed_actions.into_iter().map(String::from).collect(),
            security,
            30,
        )
    }

    #[test]
    fn name_and_schema() {
        let tool = make_tool(vec![], vec![]);
        assert_eq!(tool.name(), "github");
        let schema = tool.parameters_schema();
        assert_eq!(schema["required"], json!(["action", "repo"]));
    }

    #[tokio::test]
    async fn unknown_action_rejected() {
        let tool = make_tool(vec!["get_issue"], vec!["*"]);
        let result = tool
            .execute(json!({"action": "delete_repo", "repo": "a/b"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("Unknown action"));
    }

    #[tokio::test]
    async fn disallowed_action_rejected() {
        let tool = make_tool(vec!["get_issue"], vec!["*"]);
        let result = tool
            .execute(json!({"action": "merge_pr", "repo": "a/b", "pr_number": 1}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not enabled"));
    }

    #[tokio::test]
    async fn missing_action_param() {
        let tool = make_tool(vec!["get_issue"], vec!["*"]);
        let result = tool.execute(json!({"repo": "a/b"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("action"));
    }

    #[tokio::test]
    async fn missing_repo_param() {
        let tool = make_tool(vec!["get_issue"], vec!["*"]);
        let result = tool.execute(json!({"action": "get_issue"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("repo"));
    }

    #[tokio::test]
    async fn missing_issue_number_for_get_issue() {
        let tool = make_tool(vec!["get_issue"], vec!["*"]);
        let result = tool
            .execute(json!({"action": "get_issue", "repo": "a/b"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("issue_number"));
    }

    #[tokio::test]
    async fn missing_body_for_add_comment() {
        let tool = make_tool(vec!["add_comment"], vec!["*"]);
        let result = tool
            .execute(json!({
                "action": "add_comment",
                "repo": "a/b",
                "issue_number": 1
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("body"));
    }

    #[tokio::test]
    async fn missing_fields_for_create_pull_request() {
        let tool = make_tool(vec!["create_pull_request"], vec!["*"]);
        let result = tool
            .execute(json!({
                "action": "create_pull_request",
                "repo": "a/b",
                "title": "hi",
                "head": "feature",
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("base"));
    }

    #[tokio::test]
    async fn missing_reviewers_for_request_review() {
        let tool = make_tool(vec!["request_review"], vec!["*"]);
        let result = tool
            .execute(json!({
                "action": "request_review",
                "repo": "a/b",
                "pr_number": 1,
                "reviewers": []
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("reviewers"));
    }

    #[tokio::test]
    async fn write_actions_blocked_under_readonly() {
        for action in [
            "add_comment",
            "create_pull_request",
            "close_pr",
            "merge_pr",
            "request_review",
        ] {
            let tool = make_tool_with_autonomy(vec![action], AutonomyLevel::ReadOnly);
            // Provide enough args that the action would otherwise dispatch.
            let args = match action {
                "add_comment" => json!({
                    "action": action,
                    "repo": "myagentzero/zeroclaw",
                    "issue_number": 1,
                    "body": "hi"
                }),
                "create_pull_request" => json!({
                    "action": action,
                    "repo": "myagentzero/zeroclaw",
                    "title": "t",
                    "head": "f",
                    "base": "main"
                }),
                "close_pr" | "merge_pr" => json!({
                    "action": action,
                    "repo": "myagentzero/zeroclaw",
                    "pr_number": 1
                }),
                "request_review" => json!({
                    "action": action,
                    "repo": "myagentzero/zeroclaw",
                    "pr_number": 1,
                    "reviewers": ["alice"]
                }),
                _ => unreachable!(),
            };
            let result = tool.execute(args).await.unwrap();
            assert!(!result.success, "{action} should be blocked under ReadOnly");
        }
    }

    #[test]
    fn repo_allowlist_wildcards() {
        let t1 = make_tool(vec![], vec!["myagentzero/*"]);
        assert!(t1.repo_is_allowed("myagentzero/zeroclaw"));
        assert!(!t1.repo_is_allowed("other/repo"));

        let t2 = make_tool(vec![], vec!["*"]);
        assert!(t2.repo_is_allowed("anything/repo"));

        let t3 = make_tool(vec![], vec!["myagentzero/zeroclaw"]);
        assert!(t3.repo_is_allowed("myagentzero/zeroclaw"));
        assert!(!t3.repo_is_allowed("myagentzero/other"));

        let empty = make_tool(vec![], vec![]);
        assert!(!empty.repo_is_allowed("myagentzero/zeroclaw"));
    }

    #[test]
    fn validate_repo_rejects_bad_input() {
        assert!(validate_repo("owner/repo").is_ok());
        assert!(validate_repo("owner-1/repo_2.x").is_ok());
        assert!(validate_repo("../etc/passwd").is_err());
        assert!(validate_repo("owner only").is_err());
        assert!(validate_repo("owner").is_err());
        assert!(validate_repo("/repo").is_err());
        assert!(validate_repo("owner/").is_err());
        assert!(validate_repo("a/b/c").is_err());
    }

    #[test]
    fn validate_branch_ref_rejects_bad_input() {
        assert!(validate_branch_ref("main", "head").is_ok());
        assert!(validate_branch_ref("feature/foo", "head").is_ok());
        assert!(validate_branch_ref("", "head").is_err());
        assert!(validate_branch_ref(" ", "head").is_err());
        assert!(validate_branch_ref("..", "head").is_err());
        assert!(validate_branch_ref("a b", "head").is_err());
    }

    #[tokio::test]
    async fn create_pr_rejects_head_equals_base() {
        let tool = make_tool(vec!["create_pull_request"], vec!["myagentzero/zeroclaw"]);
        let result = tool
            .execute(json!({
                "action": "create_pull_request",
                "repo": "myagentzero/zeroclaw",
                "title": "t",
                "head": "main",
                "base": "main"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("head and base must differ"));
    }

    #[tokio::test]
    async fn merge_pr_rejects_invalid_method() {
        let tool = make_tool(vec!["merge_pr"], vec!["myagentzero/zeroclaw"]);
        let result = tool
            .execute(json!({
                "action": "merge_pr",
                "repo": "myagentzero/zeroclaw",
                "pr_number": 1,
                "merge_method": "weird"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("merge_method"));
    }

    #[tokio::test]
    async fn unauthorized_repo_rejected() {
        let tool = make_tool(vec!["get_issue"], vec!["myagentzero/zeroclaw"]);
        let result = tool
            .execute(json!({
                "action": "get_issue",
                "repo": "other/repo",
                "issue_number": 1
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not in allowed_repos"));
    }
}
