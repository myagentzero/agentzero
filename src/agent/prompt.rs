use chrono::{Local, Utc};
use std::fmt::Write;
use std::path::Path;

pub(crate) const BOOTSTRAP_MAX_CHARS: usize = 20_000;
const DATETIME_HEADER: &str = "## Current Date & Time\n\n";

/// Refresh the `## Current Date & Time` section in an existing system prompt.
/// Long-lived sessions keep a stable system prompt; this updates only the
/// timestamp payload so per-turn "current time" answers stay accurate.
pub fn refresh_prompt_datetime(prompt: &mut String, timezone_override: Option<&str>) {
    let Some(section_start) = prompt.find(DATETIME_HEADER) else {
        return;
    };

    let content_start = section_start + DATETIME_HEADER.len();
    let content_end = prompt[content_start..]
        .find('\n')
        .map(|offset| content_start + offset)
        .unwrap_or(prompt.len());

    let replacement = format_datetime(timezone_override);
    prompt.replace_range(content_start..content_end, &replacement);
}

/// Format the current datetime using the configured timezone override,
/// falling back to the system local timezone.
pub(crate) fn format_datetime(timezone_override: Option<&str>) -> String {
    if let Some(tz_name) = timezone_override {
        if let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() {
            let now = Utc::now().with_timezone(&tz);
            return format!("{} ({})", now.format("%Y-%m-%d %H:%M:%S"), tz_name);
        }
    }
    let now = Local::now();
    format!("{} ({})", now.format("%Y-%m-%d %H:%M:%S"), now.format("%Z"))
}

pub(crate) fn inject_workspace_file(
    prompt: &mut String,
    workspace_dir: &Path,
    filename: &str,
    max_chars: usize,
) {
    let path = workspace_dir.join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return;
            }
            let _ = writeln!(prompt, "### {filename}\n");
            let truncated = if trimmed.chars().count() > max_chars {
                trimmed
                    .char_indices()
                    .nth(max_chars)
                    .map(|(idx, _)| &trimmed[..idx])
                    .unwrap_or(trimmed)
            } else {
                trimmed
            };
            if truncated.len() < trimmed.len() {
                prompt.push_str(truncated);
                let _ = writeln!(
                    prompt,
                    "\n\n[... truncated at {max_chars} chars — use `read` for full file]\n"
                );
            } else {
                prompt.push_str(trimmed);
                prompt.push_str("\n\n");
            }
        }
        Err(_) => {
            let _ = writeln!(prompt, "### {filename}\n\n[File not found: {filename}]\n");
        }
    }
}

pub(crate) fn normalize_openclaw_identity_extra_file(raw: &str) -> Option<&str> {
    use std::path::{Component, Path};

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = Path::new(trimmed);
    if path.is_absolute() {
        return None;
    }

    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_datetime_returns_timestamp_with_timezone() {
        let result = format_datetime(None);
        assert!(result.chars().any(|c| c.is_ascii_digit()));
        assert!(result.contains(" ("));
        assert!(result.ends_with(')'));
    }

    #[test]
    fn format_datetime_uses_timezone_override() {
        let result = format_datetime(Some("America/Denver"));
        assert!(result.contains("America/Denver"));
    }

    #[test]
    fn format_datetime_invalid_timezone_falls_back_to_local() {
        let result = format_datetime(Some("Invalid/Timezone"));
        assert!(!result.contains("Invalid/Timezone"));
    }

    #[test]
    fn refresh_prompt_datetime_updates_timestamp_in_place() {
        let mut prompt = "## Runtime\n\nHost: test\n\n## Current Date & Time\n\n2000-01-01 00:00:00 (UTC)\n\n## Next Section".to_string();
        refresh_prompt_datetime(&mut prompt, None);

        assert!(prompt.contains("## Current Date & Time\n\n"));
        assert!(prompt.contains("\n\n## Next Section"));
        assert!(!prompt.contains("2000-01-01 00:00:00 (UTC)"));
    }

    #[test]
    fn refresh_prompt_datetime_noops_when_section_missing() {
        let mut prompt = "## Runtime\n\nHost: test".to_string();
        let original = prompt.clone();
        refresh_prompt_datetime(&mut prompt, None);
        assert_eq!(prompt, original);
    }

    #[test]
    fn refresh_prompt_datetime_with_timezone_override() {
        let mut prompt =
            "## Current Date & Time\n\n2000-01-01 00:00:00 (UTC)\n\n## Next".to_string();
        refresh_prompt_datetime(&mut prompt, Some("America/Denver"));
        assert!(!prompt.contains("2000-01-01 00:00:00 (UTC)"));
        assert!(prompt.contains("America/Denver"));
    }

    #[test]
    fn refresh_prompt_datetime_invalid_timezone_falls_back_to_local() {
        let mut prompt =
            "## Current Date & Time\n\n2000-01-01 00:00:00 (UTC)\n\n## Next".to_string();
        refresh_prompt_datetime(&mut prompt, Some("Invalid/Timezone"));
        assert!(!prompt.contains("2000-01-01 00:00:00 (UTC)"));
        assert!(!prompt.contains("Invalid/Timezone"));
    }

    #[test]
    fn inject_workspace_file_injects_content() {
        let ws = std::env::temp_dir().join(format!("zeroclaw_inject_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("TEST.md"), "hello world").unwrap();

        let mut prompt = String::new();
        inject_workspace_file(&mut prompt, &ws, "TEST.md", 20_000);
        assert!(prompt.contains("### TEST.md"));
        assert!(prompt.contains("hello world"));

        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn inject_workspace_file_truncates_long_content() {
        let ws = std::env::temp_dir().join(format!("zeroclaw_trunc_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("BIG.md"), "a".repeat(100)).unwrap();

        let mut prompt = String::new();
        inject_workspace_file(&mut prompt, &ws, "BIG.md", 10);
        assert!(prompt.contains("truncated at 10 chars"));

        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn inject_workspace_file_marks_missing_file() {
        let mut prompt = String::new();
        inject_workspace_file(&mut prompt, Path::new("/nonexistent"), "MISSING.md", 20_000);
        assert!(prompt.contains("[File not found: MISSING.md]"));
    }

    #[test]
    fn normalize_rejects_unsafe_paths() {
        assert!(normalize_openclaw_identity_extra_file("SAFE.md").is_some());
        assert!(normalize_openclaw_identity_extra_file("sub/dir/file.md").is_some());
        assert!(normalize_openclaw_identity_extra_file("../outside.md").is_none());
        assert!(normalize_openclaw_identity_extra_file("/tmp/absolute.md").is_none());
        assert!(normalize_openclaw_identity_extra_file("").is_none());
        assert!(normalize_openclaw_identity_extra_file("  ").is_none());
    }
}
