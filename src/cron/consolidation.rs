use crate::config::Config;
use crate::cron::{CronJob, CronJobPatch, DeliveryConfig, Schedule, SessionTarget, add_agent_job, list_jobs, remove_job, update_job};
use anyhow::Result;

/// Default cron expression: 3:00 AM daily.
const DEFAULT_SCHEDULE_EXPR: &str = "0 3 * * *";

/// Job name marker used to identify consolidation jobs.
pub const CONSOLIDATION_JOB_NAME: &str = "__consolidate_nightly";

/// Default prompt used as fallback when no external file is found.
/// The prompt instructs the agent to perform memory consolidation using
/// existing tools (memory_recall, memory_store, file_write).
const DEFAULT_CONSOLIDATION_PROMPT: &str = "\
You are running a nightly memory consolidation job. Your goal is to distill \
the past 24 hours of operational activity into a concise, actionable summary \
stored in long-term memory.

Follow these steps exactly:

1. Use `memory_recall` with category 'daily' and since '24h' to retrieve today's \
   session memories. Also recall category 'conversation' for today's chat observations. \
   Look for patterns, discoveries, and progress toward goals.

2. Identify and classify findings:
   - **Recurring errors**: problems that appeared more than once
   - **Successful strategies**: approaches that worked well
   - **New discoveries**: information or capabilities learned
   - **Blocked goals**: objectives that could not be completed and why

3. Synthesize a concise summary (max 500 words) of actionable learnings. \
   Focus on what should change going forward, not just what happened.

4. Use `file_read` to read MEMORY.md in the workspace directory. \
   Add today's dated section at the TOP with the top 2-3 learnings, then remove \
   any sections older than 4-5 days (keep only recent entries). Use `file_write` \
   to save the updated file. Format for new section:
   ```
   ## Learnings: YYYY-MM-DD
   1. <learning 1>
   2. <learning 2>
   3. <learning 3> (optional)
   ```

If there is no meaningful activity to consolidate (no conversations, no daily memories), \
skip updating the MEMORY.md file.

5. Output the full consolidation summary as your final response text. \
   An external delivery system reads your response to announce the results. \
   If there was no meaningful activity (step above), respond with exactly `NO_REPLY` \
   and nothing else — the delivery system will skip the announcement.";

/// Load the consolidation prompt from the workspace file, falling back to the
/// built-in default if the file doesn't exist or can't be read.
fn load_consolidation_prompt(config: &Config) -> String {
    let prompt_filename = config
        .consolidation
        .prompt_file
        .as_deref()
        .unwrap_or("CONSOLIDATION.md");

    let prompt_path = config.workspace_dir.join(prompt_filename);

    match std::fs::read_to_string(&prompt_path) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => DEFAULT_CONSOLIDATION_PROMPT.to_string(),
    }
}

/// Create a default CONSOLIDATION.md if it doesn't exist.
pub async fn ensure_consolidation_file(workspace_dir: &std::path::Path) -> anyhow::Result<()> {
    let path = workspace_dir.join("CONSOLIDATION.md");
    if !path.exists() {
        tokio::fs::write(&path, DEFAULT_CONSOLIDATION_PROMPT).await?;
    }
    Ok(())
}

/// Build a delivery config for the consolidation job from config.
/// Returns `None` when no delivery channel is configured (no announcement).
fn resolve_delivery_config(config: &Config) -> Option<DeliveryConfig> {
    let channel = config.consolidation.delivery_channel.as_deref()?.to_string();

    let to = if let Some(explicit) = config.consolidation.delivery_to.as_deref() {
        explicit.to_string()
    } else {
        resolve_default_target(config, &channel)?
    };

    Some(DeliveryConfig {
        mode: "announce".to_string(),
        channel: Some(channel),
        to: Some(to),
        best_effort: config.consolidation.delivery_best_effort,
    })
}

/// Resolve the default delivery target for the given channel type from channel config.
fn resolve_default_target(config: &Config, channel: &str) -> Option<String> {
    match channel.to_ascii_lowercase().as_str() {
        "slack" => {
            let sl = config.channels_config.slack.as_ref()?;
            if let Some(id) = sl.channel_ids.first() {
                return Some(id.clone());
            }
            let id = sl.channel_id.as_deref()?;
            if id.is_empty() || id == "*" {
                tracing::warn!("consolidation delivery_channel=slack but no usable channel_id found");
                return None;
            }
            Some(id.to_string())
        }
        "discord" => {
            let dc = config.channels_config.discord.as_ref()?;
            dc.guild_id.clone()
        }
        other => {
            tracing::warn!("consolidation: cannot auto-resolve delivery target for channel '{other}'");
            None
        }
    }
}

/// Create a nightly memory consolidation cron agent job.
///
/// Pulls configuration from `config.consolidation` (schedule, timezone, light_context).
/// Job type: agent with `__consolidate_nightly` marker in the name.
/// Session target: isolated (does not disturb main sessions).
pub fn create_consolidation_job(config: &Config) -> Result<CronJob> {
    create_consolidation_job_with_schedule(
        config,
        &config.consolidation.schedule,
        config.consolidation.timezone.clone(),
        config.consolidation.light_context,
    )
}

/// Create a consolidation job with a custom cron expression, timezone, and light_context setting.
pub fn create_consolidation_job_with_schedule(
    config: &Config,
    cron_expr: &str,
    tz: Option<String>,
    light_context: bool,
) -> Result<CronJob> {
    let schedule = Schedule::Cron {
        expr: cron_expr.into(),
        tz,
    };
    let prompt = load_consolidation_prompt(config);

    let delivery = resolve_delivery_config(config);

    add_agent_job(
        config,
        Some(CONSOLIDATION_JOB_NAME.into()),
        schedule,
        &prompt,
        SessionTarget::Isolated,
        None,  // use default model
        delivery,
        false, // recurring job — do not delete after run
        light_context,
    )
}

/// Ensure the consolidation cron job exists in the store.
/// If a job named `__consolidate_nightly` already exists, update its schedule/prompt/light_context.
/// If it doesn't exist, create it.
pub fn ensure_consolidation_job(config: &Config) -> Result<()> {
    let jobs = list_jobs(config)?;
    let existing = jobs
        .iter()
        .find(|j| j.name.as_deref() == Some(CONSOLIDATION_JOB_NAME));

    if let Some(job) = existing {
        // Update existing job with current config values
        let prompt = load_consolidation_prompt(config);
        let schedule = Schedule::Cron {
            expr: config.consolidation.schedule.clone(),
            tz: config.consolidation.timezone.clone(),
        };
        let patch = CronJobPatch {
            schedule: Some(schedule),
            prompt: Some(prompt),
            light_context: Some(config.consolidation.light_context),
            enabled: Some(true),
            delivery: resolve_delivery_config(config),
            ..Default::default()
        };
        update_job(config, &job.id, patch)?;
    } else {
        create_consolidation_job(config)?;
    }
    Ok(())
}

/// Remove the consolidation job from the cron store if it exists.
pub fn remove_consolidation_job(config: &Config) -> Result<()> {
    let jobs = list_jobs(config)?;
    if let Some(job) = jobs
        .iter()
        .find(|j| j.name.as_deref() == Some(CONSOLIDATION_JOB_NAME))
    {
        remove_job(config, &job.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::{JobType, Schedule, SessionTarget};
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        config
    }

    #[test]
    fn create_consolidation_job_produces_valid_job() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let job = create_consolidation_job(&config).unwrap();

        assert_eq!(job.name.as_deref(), Some(CONSOLIDATION_JOB_NAME));
        assert_eq!(job.job_type, JobType::Agent);
        assert_eq!(job.session_target, SessionTarget::Isolated);
        assert!(!job.delete_after_run);
        assert!(job.enabled);
    }

    #[test]
    fn create_consolidation_job_uses_correct_schedule() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let job = create_consolidation_job(&config).unwrap();

        match &job.schedule {
            Schedule::Cron { expr, tz } => {
                assert_eq!(expr, DEFAULT_SCHEDULE_EXPR);
                assert!(tz.is_none());
            }
            other => panic!("Expected Cron schedule, got {other:?}"),
        }
    }

    #[test]
    fn create_consolidation_job_prompt_contains_key_instructions() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let job = create_consolidation_job(&config).unwrap();
        let prompt = job.prompt.expect("consolidation job must have a prompt");

        assert!(
            prompt.contains("memory_recall"),
            "prompt should instruct use of memory_recall"
        );
        assert!(
            prompt.contains("file_read"),
            "prompt should instruct use of file_read"
        );
        assert!(
            prompt.contains("file_write"),
            "prompt should instruct use of file_write"
        );
        assert!(
            prompt.contains("MEMORY.md"),
            "prompt should mention MEMORY.md"
        );
    }

    #[test]
    fn create_consolidation_job_with_custom_schedule_applies_tz() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let job = create_consolidation_job_with_schedule(
            &config,
            "0 4 * * *",
            Some("America/New_York".into()),
            false,
        )
        .unwrap();

        match &job.schedule {
            Schedule::Cron { expr, tz } => {
                assert_eq!(expr, "0 4 * * *");
                assert_eq!(tz.as_deref(), Some("America/New_York"));
            }
            other => panic!("Expected Cron schedule, got {other:?}"),
        }
    }

    #[test]
    fn load_consolidation_prompt_uses_file_when_present() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let custom_prompt = "Custom consolidation instructions here";
        std::fs::write(
            config.workspace_dir.join("CONSOLIDATION.md"),
            custom_prompt,
        )
        .unwrap();

        let job = create_consolidation_job(&config).unwrap();
        assert_eq!(job.prompt.as_deref(), Some(custom_prompt));
    }

    #[test]
    fn load_consolidation_prompt_falls_back_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        // No CONSOLIDATION.md created

        let job = create_consolidation_job(&config).unwrap();
        let prompt = job.prompt.unwrap();
        assert!(prompt.contains("memory_recall"));
        assert!(prompt.contains("file_write"));
    }

    #[test]
    fn create_consolidation_job_respects_light_context_config() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.light_context = true;

        let job = create_consolidation_job(&config).unwrap();
        assert!(job.light_context);
    }

    #[test]
    fn ensure_consolidation_job_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.enabled = true;

        ensure_consolidation_job(&config).unwrap();
        ensure_consolidation_job(&config).unwrap();

        let jobs: Vec<_> = list_jobs(&config)
            .unwrap()
            .into_iter()
            .filter(|j| j.name.as_deref() == Some(CONSOLIDATION_JOB_NAME))
            .collect();
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn create_consolidation_job_prompt_contains_step_6_output_instruction() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let job = create_consolidation_job(&config).unwrap();
        let prompt = job.prompt.unwrap();

        assert!(
            prompt.contains("NO_REPLY"),
            "prompt should mention NO_REPLY sentinel"
        );
        assert!(
            prompt.contains("final response"),
            "prompt should instruct agent to output summary as final response"
        );
    }

    #[test]
    fn resolve_delivery_returns_none_when_not_configured() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        assert!(resolve_delivery_config(&config).is_none());
    }

    #[test]
    fn resolve_delivery_from_explicit_channel_and_to() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.delivery_channel = Some("slack".into());
        config.consolidation.delivery_to = Some("C123456".into());

        let delivery = resolve_delivery_config(&config).unwrap();
        assert_eq!(delivery.mode, "announce");
        assert_eq!(delivery.channel.as_deref(), Some("slack"));
        assert_eq!(delivery.to.as_deref(), Some("C123456"));
        assert!(delivery.best_effort);
    }

    #[test]
    fn resolve_delivery_auto_resolves_slack_channel_id() {
        use crate::config::schema::{ChannelsConfig, SlackConfig};

        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.delivery_channel = Some("slack".into());
        config.channels_config = ChannelsConfig {
            slack: Some(SlackConfig {
                bot_token: "xoxb-test".into(),
                app_token: None,
                channel_id: Some("CAUTO".into()),
                channel_ids: vec![],
                allowed_users: vec![],
                group_reply: None,
            }),
            ..ChannelsConfig::default()
        };

        let delivery = resolve_delivery_config(&config).unwrap();
        assert_eq!(delivery.to.as_deref(), Some("CAUTO"));
    }

    #[test]
    fn resolve_delivery_prefers_channel_ids_over_channel_id() {
        use crate::config::schema::{ChannelsConfig, SlackConfig};

        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.delivery_channel = Some("slack".into());
        config.channels_config = ChannelsConfig {
            slack: Some(SlackConfig {
                bot_token: "xoxb-test".into(),
                app_token: None,
                channel_id: Some("CSINGULAR".into()),
                channel_ids: vec!["CFIRST".into(), "CSECOND".into()],
                allowed_users: vec![],
                group_reply: None,
            }),
            ..ChannelsConfig::default()
        };

        let delivery = resolve_delivery_config(&config).unwrap();
        assert_eq!(delivery.to.as_deref(), Some("CFIRST"));
    }

    #[test]
    fn resolve_delivery_skips_wildcard_channel_id() {
        use crate::config::schema::{ChannelsConfig, SlackConfig};

        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.delivery_channel = Some("slack".into());
        config.channels_config = ChannelsConfig {
            slack: Some(SlackConfig {
                bot_token: "xoxb-test".into(),
                app_token: None,
                channel_id: Some("*".into()),
                channel_ids: vec![],
                allowed_users: vec![],
                group_reply: None,
            }),
            ..ChannelsConfig::default()
        };

        assert!(resolve_delivery_config(&config).is_none());
    }

    #[test]
    fn create_consolidation_job_includes_delivery_when_configured() {
        use crate::config::schema::{ChannelsConfig, SlackConfig};

        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.delivery_channel = Some("slack".into());
        config.consolidation.delivery_to = Some("CDIRECT".into());
        config.channels_config = ChannelsConfig {
            slack: Some(SlackConfig {
                bot_token: "xoxb-test".into(),
                app_token: None,
                channel_id: None,
                channel_ids: vec![],
                allowed_users: vec![],
                group_reply: None,
            }),
            ..ChannelsConfig::default()
        };

        let job = create_consolidation_job(&config).unwrap();
        assert_eq!(job.delivery.mode, "announce");
        assert_eq!(job.delivery.channel.as_deref(), Some("slack"));
        assert_eq!(job.delivery.to.as_deref(), Some("CDIRECT"));
    }

    #[test]
    fn ensure_consolidation_job_patches_delivery_on_update() {
        use crate::config::schema::{ChannelsConfig, SlackConfig};

        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.consolidation.enabled = true;
        // Create without delivery first
        ensure_consolidation_job(&config).unwrap();

        // Now add delivery config and update
        config.consolidation.delivery_channel = Some("slack".into());
        config.consolidation.delivery_to = Some("CUPDATE".into());
        config.channels_config = ChannelsConfig {
            slack: Some(SlackConfig {
                bot_token: "xoxb-test".into(),
                app_token: None,
                channel_id: None,
                channel_ids: vec![],
                allowed_users: vec![],
                group_reply: None,
            }),
            ..ChannelsConfig::default()
        };
        ensure_consolidation_job(&config).unwrap();

        let jobs: Vec<_> = list_jobs(&config)
            .unwrap()
            .into_iter()
            .filter(|j| j.name.as_deref() == Some(CONSOLIDATION_JOB_NAME))
            .collect();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].delivery.mode, "announce");
        assert_eq!(jobs[0].delivery.to.as_deref(), Some("CUPDATE"));
    }
}
