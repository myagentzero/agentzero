use crate::cron::Schedule;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, Utc};
use cron::Schedule as CronExprSchedule;
use serde_json::Value;
use std::str::FromStr;

const NAIVE_AT_FORMATS: &[&str] = &[
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%d %H:%M",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%dT%H:%M",
];

/// Parse `schedule.at` from RFC3339 or common lenient formats (interpreted as UTC).
pub fn parse_at_timestamp_lenient(raw: &str) -> Result<DateTime<Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("schedule.at must be a non-empty timestamp");
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(parsed.with_timezone(&Utc));
    }

    for fmt in NAIVE_AT_FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            return Ok(naive.and_utc());
        }
    }

    Err(anyhow!(schedule_at_parse_hint(trimmed)))
}

/// Build a model-friendly hint for invalid `schedule.at` values.
pub fn schedule_at_parse_hint(raw_at: &str) -> String {
    let looks_like_space_separated = raw_at.contains(' ') && !raw_at.contains('T');
    if looks_like_space_separated {
        format!(
            "schedule.at must be RFC3339 (e.g. \"2026-06-29T09:00:00Z\"). \
Got \"{raw_at}\" — use ISO-8601 with 'T' and a timezone, or \"YYYY-MM-DD HH:MM:SS\" (interpreted as UTC)"
        )
    } else {
        format!(
            "schedule.at must be RFC3339 (e.g. \"2026-06-29T09:00:00Z\" or \"2026-06-29T09:00:00-07:00\"). Got \"{raw_at}\""
        )
    }
}

fn normalize_schedule_json_value(mut value: Value) -> Result<Value> {
    let Some(obj) = value.as_object_mut() else {
        anyhow::bail!("schedule must be a JSON object");
    };

    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    if kind.as_deref() != Some("at") {
        return Ok(value);
    }

    let Some(at_raw) = obj.get("at").and_then(Value::as_str) else {
        anyhow::bail!("schedule.kind='at' requires a string schedule.at timestamp");
    };

    let normalized = parse_at_timestamp_lenient(at_raw)?.to_rfc3339();
    obj.insert("at".to_string(), Value::String(normalized));
    Ok(value)
}

/// Parse schedule JSON from tools/API, normalizing lenient `at` timestamps first.
pub fn parse_schedule_json(value: Value) -> Result<Schedule> {
    let raw_at = value.get("at").and_then(Value::as_str).map(str::to_string);
    let normalized = normalize_schedule_json_value(value).with_context(|| {
        raw_at
            .as_deref()
            .map(schedule_at_parse_hint)
            .unwrap_or_else(|| "Invalid schedule object".to_string())
    })?;
    serde_json::from_value(normalized).with_context(|| {
        raw_at
            .as_deref()
            .map(schedule_at_parse_hint)
            .unwrap_or_else(|| "Invalid schedule object".to_string())
    })
}

/// Format a deserialization failure into a model-actionable error message.
pub fn format_schedule_parse_error(value: &Value, err: &serde_json::Error) -> String {
    if let Some(raw_at) = value.get("at").and_then(Value::as_str) {
        return format!(
            "Invalid schedule: {} ({err})",
            schedule_at_parse_hint(raw_at)
        );
    }
    format!("Invalid schedule: {err}")
}

pub fn next_run_for_schedule(schedule: &Schedule, from: DateTime<Utc>) -> Result<DateTime<Utc>> {
    match schedule {
        Schedule::Cron { expr, tz } => {
            let normalized = normalize_expression(expr)?;
            let cron = CronExprSchedule::from_str(&normalized)
                .with_context(|| format!("Invalid cron expression: {expr}"))?;

            if let Some(tz_name) = tz {
                let timezone = chrono_tz::Tz::from_str(tz_name)
                    .with_context(|| format!("Invalid IANA timezone: {tz_name}"))?;
                let localized_from = from.with_timezone(&timezone);
                let next_local = cron.after(&localized_from).next().ok_or_else(|| {
                    anyhow::anyhow!("No future occurrence for expression: {expr}")
                })?;
                Ok(next_local.with_timezone(&Utc))
            } else {
                cron.after(&from)
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("No future occurrence for expression: {expr}"))
            }
        }
        Schedule::At { at } => Ok(*at),
        Schedule::Every { every_ms } => {
            if *every_ms == 0 {
                anyhow::bail!("Invalid schedule: every_ms must be > 0");
            }
            let ms = i64::try_from(*every_ms).context("every_ms is too large")?;
            let delta = ChronoDuration::milliseconds(ms);
            from.checked_add_signed(delta)
                .ok_or_else(|| anyhow::anyhow!("every_ms overflowed DateTime"))
        }
    }
}

pub fn validate_schedule(schedule: &Schedule, now: DateTime<Utc>) -> Result<()> {
    match schedule {
        Schedule::Cron { expr, .. } => {
            let _ = normalize_expression(expr)?;
            let _ = next_run_for_schedule(schedule, now)?;
            Ok(())
        }
        Schedule::At { at } => {
            if *at <= now {
                anyhow::bail!("Invalid schedule: 'at' must be in the future");
            }
            Ok(())
        }
        Schedule::Every { every_ms } => {
            if *every_ms == 0 {
                anyhow::bail!("Invalid schedule: every_ms must be > 0");
            }
            Ok(())
        }
    }
}

pub fn schedule_cron_expression(schedule: &Schedule) -> Option<String> {
    match schedule {
        Schedule::Cron { expr, .. } => Some(expr.clone()),
        _ => None,
    }
}

pub fn normalize_expression(expression: &str) -> Result<String> {
    let expression = expression.trim();
    let field_count = expression.split_whitespace().count();

    match field_count {
        // standard crontab syntax: minute hour day month weekday
        5 => Ok(format!("0 {expression}")),
        // crate-native syntax includes seconds (+ optional year)
        6 | 7 => Ok(expression.to_string()),
        _ => anyhow::bail!(
            "Invalid cron expression: {expression} (expected 5, 6, or 7 fields, got {field_count})"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn next_run_for_schedule_supports_every_and_at() {
        let now = Utc::now();
        let every = Schedule::Every { every_ms: 60_000 };
        let next = next_run_for_schedule(&every, now).unwrap();
        assert!(next > now);

        let at = now + ChronoDuration::minutes(10);
        let at_schedule = Schedule::At { at };
        let next_at = next_run_for_schedule(&at_schedule, now).unwrap();
        assert_eq!(next_at, at);
    }

    #[test]
    fn next_run_for_schedule_supports_timezone() {
        let from = Utc.with_ymd_and_hms(2026, 2, 16, 0, 0, 0).unwrap();
        let schedule = Schedule::Cron {
            expr: "0 9 * * *".into(),
            tz: Some("America/Los_Angeles".into()),
        };

        let next = next_run_for_schedule(&schedule, from).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 2, 16, 17, 0, 0).unwrap());
    }

    #[test]
    fn parse_at_timestamp_lenient_accepts_rfc3339_and_space_separated_utc() {
        let rfc = parse_at_timestamp_lenient("2026-06-29T09:00:00Z").unwrap();
        assert_eq!(rfc, Utc.with_ymd_and_hms(2026, 6, 29, 9, 0, 0).unwrap());

        let spaced = parse_at_timestamp_lenient("2026-06-29 09:00:00").unwrap();
        assert_eq!(spaced, Utc.with_ymd_and_hms(2026, 6, 29, 9, 0, 0).unwrap());
    }

    #[test]
    fn parse_schedule_json_normalizes_lenient_at_payload() {
        let schedule = parse_schedule_json(serde_json::json!({
            "kind": "at",
            "at": "2026-06-29 09:00:00"
        }))
        .unwrap();
        assert_eq!(
            schedule,
            Schedule::At {
                at: Utc.with_ymd_and_hms(2026, 6, 29, 9, 0, 0).unwrap()
            }
        );
    }

    #[test]
    fn parse_schedule_json_rejects_unparseable_at_with_hint() {
        let err = parse_schedule_json(serde_json::json!({
            "kind": "at",
            "at": "not-a-date"
        }))
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("RFC3339"));
        assert!(message.contains("not-a-date"));
    }
}
