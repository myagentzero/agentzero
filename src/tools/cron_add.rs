use super::traits::{Tool, ToolResult};
use crate::config::Config;
use crate::cron::{self, DeliveryConfig, JobType, Schedule, SessionTarget};
use crate::security::SecurityPolicy;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

pub struct CronAddTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
}

const MIN_AGENT_EVERY_MS: u64 = 5 * 60 * 1000;

impl CronAddTool {
    pub fn new(config: Arc<Config>, security: Arc<SecurityPolicy>) -> Self {
        Self { config, security }
    }

    fn enforce_mutation_allowed(&self, action: &str) -> Option<ToolResult> {
        if !self.security.can_act() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Security policy: read-only mode, cannot perform '{action}'"
                )),
            });
        }

        if self.security.is_rate_limited() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".to_string()),
            });
        }

        if !self.security.record_action() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".to_string()),
            });
        }

        None
    }
}

#[async_trait]
impl Tool for CronAddTool {
    fn name(&self) -> &str {
        "cron_add"
    }

    fn description(&self) -> &str {
        "Schedule shell commands or agent prompts."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short readable job name."
                },
                "schedule": {
                    "type": "object",
                    "description": "When to run. One of: {kind:'cron',expr,tz?} | {kind:'at',at} | {kind:'every',every_ms}.",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["cron", "at", "every"],
                            "description": "Schedule type."
                        },
                        "expr": {
                            "type": "string",
                            "description": "Cron expression (5-7 fields). Required when kind='cron'."
                        },
                        "tz": {
                            "type": "string",
                            "description": "Optional IANA timezone for kind='cron' (e.g. America/Los_Angeles)."
                        },
                        "at": {
                            "type": "string",
                            "format": "date-time",
                            "description": "RFC3339 timestamp for kind='at' (e.g. 2026-06-29T09:00:00Z). YYYY-MM-DD HH:MM:SS is accepted as UTC."
                        },
                        "every_ms": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Interval in milliseconds for kind='every'."
                        }
                    },
                    "examples": [
                        { "kind": "at", "at": "2026-06-29T09:00:00Z" },
                        { "kind": "cron", "expr": "0 9 * * *", "tz": "America/Los_Angeles" },
                        { "kind": "every", "every_ms": 3600000 }
                    ]
                },
                "job_type": {
                    "type": "string",
                    "enum": ["shell", "agent"],
                    "description": "shell runs command; agent runs an LLM prompt. Inferred from command vs prompt when omitted."
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to execute. Required for shell jobs."
                },
                "prompt": {
                    "type": "string",
                    "description": "Agent instruction to run on schedule. Required for agent jobs."
                },
                "session_target": {
                    "type": "string",
                    "enum": ["isolated", "main"],
                    "description": "Agent session target. Default: isolated."
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override for agent jobs (e.g. hint:fast)."
                },
                "recurring_confirmed": {
                    "type": "boolean",
                    "description": "Must be true for recurring agent schedules (kind cron or every).",
                    "default": false
                },
                "delivery": {
                    "type": "object",
                    "description": "Optional output delivery for agent jobs.",
                    "properties": {
                        "mode": { "type": "string", "enum": ["none", "announce"], "description": "Delivery mode." },
                        "channel": { "type": "string", "enum": ["notion", "slack", "email"], "description": "Delivery channel." },
                        "to": { "type": "string", "description": "Channel target (ID/address)." },
                        "best_effort": { "type": "boolean", "description": "If true, ignore delivery failure." }
                    },
                    "examples": [
                        { "mode": "announce", "channel": "slack", "to": "CHANNEL_ID" }
                    ]
                },
                "light_context": {
                    "type": "boolean",
                    "description": "Use small context for simple agent reminders to save tokens.",
                    "default": true
                },
                "delete_after_run": {
                    "type": "boolean",
                    "description": "Delete job after it runs. Defaults to true for kind='at'."
                },
                "approved": {
                    "type": "boolean",
                    "description": "Required for medium/high-risk shell jobs in supervised mode.",
                    "default": false
                }
            },
            "required": ["schedule"],
            "examples": [
                {
                    "name": "one-shot-reminder",
                    "job_type": "agent",
                    "prompt": "Send a short reminder message.",
                    "schedule": { "kind": "at", "at": "2026-06-29T09:00:00Z" },
                    "light_context": true,
                    "delivery": { "mode": "announce", "channel": "slack", "to": "CHANNEL_ID" }
                }
            ]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if !self.config.cron.enabled {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("cron is disabled by config (cron.enabled=false)".to_string()),
            });
        }

        let schedule = match args.get("schedule") {
            Some(v) => match cron::parse_schedule_json(v.clone()) {
                Ok(schedule) => schedule,
                Err(e) => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    });
                }
            },
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing 'schedule' parameter".to_string()),
                });
            }
        };

        let name = args
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let job_type = match args.get("job_type").and_then(serde_json::Value::as_str) {
            Some("agent") => JobType::Agent,
            Some("shell") => JobType::Shell,
            Some(other) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid job_type: {other}")),
                });
            }
            None => {
                if args.get("prompt").is_some() {
                    JobType::Agent
                } else {
                    JobType::Shell
                }
            }
        };

        let default_delete_after_run = matches!(schedule, Schedule::At { .. });
        let delete_after_run = args
            .get("delete_after_run")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_delete_after_run);
        let approved = args
            .get("approved")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let result = match job_type {
            JobType::Shell => {
                let command = match args.get("command").and_then(serde_json::Value::as_str) {
                    Some(command) if !command.trim().is_empty() => command,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'command' for shell job".to_string()),
                        });
                    }
                };

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
                }

                cron::add_shell_job_with_approval(&self.config, name, schedule, command, approved)
            }
            JobType::Agent => {
                let prompt = match args.get("prompt").and_then(serde_json::Value::as_str) {
                    Some(prompt) if !prompt.trim().is_empty() => prompt,
                    _ => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'prompt' for agent job".to_string()),
                        });
                    }
                };

                let session_target = match args.get("session_target") {
                    Some(v) => match serde_json::from_value::<SessionTarget>(v.clone()) {
                        Ok(target) => target,
                        Err(e) => {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!("Invalid session_target: {e}")),
                            });
                        }
                    },
                    None => SessionTarget::Isolated,
                };

                let model = args
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let recurring_confirmed = args
                    .get("recurring_confirmed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);

                match &schedule {
                    Schedule::Every { every_ms } => {
                        if !recurring_confirmed {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(
                                    "Agent jobs with recurring schedules require recurring_confirmed=true. \
For one-time reminders, use schedule.kind='at' with an RFC3339 timestamp."
                                        .to_string(),
                                ),
                            });
                        }
                        if *every_ms < MIN_AGENT_EVERY_MS {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!(
                                    "Agent schedule.kind='every' must be >= {MIN_AGENT_EVERY_MS} ms (5 minutes)"
                                )),
                            });
                        }
                    }
                    Schedule::Cron { .. } => {
                        if !recurring_confirmed {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(
                                    "Agent jobs with recurring schedules require recurring_confirmed=true. \
For one-time reminders, use schedule.kind='at' with an RFC3339 timestamp."
                                        .to_string(),
                                ),
                            });
                        }
                    }
                    Schedule::At { .. } => {}
                }

                let light_context = args
                    .get("light_context")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);

                let delivery = match args.get("delivery") {
                    Some(v) => match serde_json::from_value::<DeliveryConfig>(v.clone()) {
                        Ok(cfg) => Some(cfg),
                        Err(e) => {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!("Invalid delivery config: {e}")),
                            });
                        }
                    },
                    None => None,
                };

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
                }

                cron::add_agent_job(
                    &self.config,
                    name,
                    schedule,
                    prompt,
                    session_target,
                    model,
                    delivery,
                    delete_after_run,
                    light_context,
                )
            }
        };

        match result {
            Ok(job) => Ok(ToolResult {
                success: true,
                output: serde_json::to_string_pretty(&json!({
                    "id": job.id,
                    "name": job.name,
                    "job_type": job.job_type,
                    "schedule": job.schedule,
                    "next_run": job.next_run,
                    "enabled": job.enabled
                }))?,
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
    use crate::config::Config;
    use crate::security::AutonomyLevel;
    use tempfile::TempDir;

    async fn test_config(tmp: &TempDir) -> Arc<Config> {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        tokio::fs::create_dir_all(&config.workspace_dir)
            .await
            .unwrap();
        Arc::new(config)
    }

    fn test_security(cfg: &Config) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy::from_config(
            &cfg.autonomy,
            &cfg.workspace_dir,
        ))
    }

    #[tokio::test]
    async fn adds_shell_job() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));
        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "echo ok"
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("next_run"));
    }

    #[tokio::test]
    async fn blocks_disallowed_shell_command() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.allowed_commands = vec!["echo".into()];
        config.autonomy.level = AutonomyLevel::Supervised;
        tokio::fs::create_dir_all(&config.workspace_dir)
            .await
            .unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "curl https://example.com"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("not allowed"));
    }

    #[tokio::test]
    async fn blocks_mutation_in_read_only_mode() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.level = AutonomyLevel::ReadOnly;
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "echo ok"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        let error = result.error.unwrap_or_default();
        assert!(error.contains("read-only") || error.contains("not allowed"));
    }

    #[tokio::test]
    async fn blocks_add_when_rate_limited() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.level = AutonomyLevel::Full;
        config.autonomy.max_actions_per_hour = 0;
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "echo ok"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("Rate limit exceeded")
        );
        assert!(cron::list_jobs(&cfg).unwrap().is_empty());
    }

    #[tokio::test]
    async fn medium_risk_shell_command_requires_approval() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.autonomy.allowed_commands = vec!["touch".into()];
        config.autonomy.level = AutonomyLevel::Supervised;
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        let cfg = Arc::new(config);
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let denied = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "touch cron-approval-test"
            }))
            .await
            .unwrap();
        assert!(!denied.success);
        assert!(
            denied
                .error
                .unwrap_or_default()
                .contains("explicit approval")
        );

        let approved = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "shell",
                "command": "touch cron-approval-test",
                "approved": true
            }))
            .await
            .unwrap();
        assert!(approved.success, "{:?}", approved.error);
    }

    #[tokio::test]
    async fn rejects_invalid_schedule() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "every", "every_ms": 0 },
                "job_type": "shell",
                "command": "echo nope"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("every_ms must be > 0")
        );
    }

    #[tokio::test]
    async fn agent_job_requires_prompt() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "agent"
            }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("Missing 'prompt'")
        );
    }

    #[tokio::test]
    async fn agent_every_requires_recurring_confirmation() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "every", "every_ms": 300000 },
                "job_type": "agent",
                "prompt": "Send me a recurring status update"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("recurring_confirmed=true")
        );
    }

    #[tokio::test]
    async fn agent_cron_requires_recurring_confirmation() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "cron", "expr": "*/5 * * * *" },
                "job_type": "agent",
                "prompt": "Send recurring reminders"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("recurring_confirmed=true")
        );
    }

    #[tokio::test]
    async fn agent_every_rejects_high_frequency_intervals() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "every", "every_ms": 60000 },
                "job_type": "agent",
                "prompt": "Send me updates frequently",
                "recurring_confirmed": true
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("must be >= 300000 ms")
        );
    }

    #[tokio::test]
    async fn agent_every_with_explicit_confirmation_succeeds() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "every", "every_ms": 300000 },
                "job_type": "agent",
                "prompt": "Share a heartbeat summary",
                "recurring_confirmed": true
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("next_run"));
    }

    #[tokio::test]
    async fn agent_at_job_with_light_context_stores_flag() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let result = tool
            .execute(json!({
                "schedule": { "kind": "at", "at": at },
                "job_type": "agent",
                "prompt": "Quick weather check",
                "light_context": true
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        let output: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let job_id = output["id"].as_str().unwrap();

        let job = crate::cron::get_job(&cfg, job_id).unwrap();
        assert!(job.light_context);
    }

    #[tokio::test]
    async fn agent_at_job_accepts_space_separated_timestamp() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let at = (chrono::Utc::now() + chrono::Duration::days(2))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let result = tool
            .execute(json!({
                "name": "one-shot-reminder",
                "prompt": "Send a short reminder message.",
                "schedule": { "kind": "at", "at": at },
                "delivery": {
                    "mode": "announce",
                    "channel": "slack",
                    "to": "CHANNEL_ID"
                }
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("next_run"));
    }

    #[tokio::test]
    async fn invalid_at_timestamp_returns_actionable_error() {
        let tmp = TempDir::new().unwrap();
        let cfg = test_config(&tmp).await;
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));

        let result = tool
            .execute(json!({
                "schedule": { "kind": "at", "at": "tomorrow morning" },
                "job_type": "agent",
                "prompt": "Ping user"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        let error = result.error.unwrap_or_default();
        assert!(error.contains("RFC3339"));
        assert!(error.contains("tomorrow morning"));
    }

    #[test]
    fn parameters_schema_documents_rfc3339_and_agent_example() {
        let tmp = TempDir::new().unwrap();
        let cfg = std::sync::Arc::new(Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        });
        let tool = CronAddTool::new(cfg.clone(), test_security(&cfg));
        let schema = tool.parameters_schema();
        let schedule_at = &schema["properties"]["schedule"]["properties"]["at"];
        assert!(
            schedule_at["description"]
                .as_str()
                .unwrap()
                .contains("RFC3339")
        );
        assert!(
            schema["examples"][0]["schedule"]["at"]
                .as_str()
                .unwrap()
                .contains('T')
        );
        assert_eq!(schema["examples"][0]["job_type"], "agent");
    }
}
