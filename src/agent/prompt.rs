use chrono::{Local, Utc};
use std::fmt::Write;
use std::path::Path;

pub(crate) const BOOTSTRAP_MAX_CHARS: usize = 20_000;
const COMPACT_BOOTSTRAP_MAX_CHARS: usize = 200;
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
            return format!("{} ({})", now.format("%A, %Y-%m-%d %H:%M:%S"), tz_name);
        }
    }
    let now = Local::now();
    format!(
        "{} ({})",
        now.format("%A, %Y-%m-%d %H:%M:%S"),
        now.format("%Z")
    )
}

fn shift_bootstrap_heading_line(line: &str) -> String {
    let trimmed_start = line.trim_start();
    if trimmed_start.is_empty() || !trimmed_start.starts_with('#') {
        return line.to_string();
    }

    let leading_ws = &line[..line.len() - trimmed_start.len()];
    let hash_count = trimmed_start.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&hash_count) {
        return line.to_string();
    }

    let rest = &trimmed_start[hash_count..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return line.to_string();
    }

    let new_level = match hash_count {
        1 => 4,
        2 => 5,
        3 => 6,
        level => level,
    };

    format!("{leading_ws}{}{rest}", "#".repeat(new_level))
}

fn shift_bootstrap_headings(content: &str) -> String {
    content
        .lines()
        .map(shift_bootstrap_heading_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_at_chars<'a>(content: &'a str, max_chars: usize) -> (&'a str, bool) {
    match content.char_indices().nth(max_chars) {
        Some((idx, _)) => (&content[..idx], true),
        None => (content, false),
    }
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
            let processed = shift_bootstrap_headings(content.trim());
            if processed.is_empty() {
                return;
            }
            let (body, truncated) = truncate_at_chars(&processed, max_chars);
            let _ = writeln!(prompt, "### {filename}\n{body}");
            if truncated {
                let _ = writeln!(
                    prompt,
                    "\n[... truncated at {max_chars} chars — use `file_read` tool for full file]\n"
                );
            }
            prompt.push_str("\n");
        }
        Err(_) => {
            let _ = writeln!(prompt, "### {filename}\n\n[File not found: {filename}]\n");
        }
    }
}

pub(crate) fn normalize_identity_extra_file(raw: &str) -> Option<&str> {
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

fn load_bootstrap_files(
    prompt: &mut String,
    workspace_dir: &Path,
    max_chars_per_file: usize,
    identity_config: Option<&crate::config::IdentityConfig>,
) {
    prompt.push_str("Files read from the workspace define behavior and context.\n\n");

    let bootstrap_files = ["AGENTS.md", "TOOLS.md", "USER.md"];

    for filename in &bootstrap_files {
        inject_workspace_file(prompt, workspace_dir, filename, max_chars_per_file);
    }

    for filename in ["BOOTSTRAP.md", "MEMORY.md"] {
        if workspace_dir.join(filename).exists() {
            inject_workspace_file(prompt, workspace_dir, filename, max_chars_per_file);
        }
    }

    let extra_files = identity_config.map_or(&[][..], |cfg| cfg.extra_files.as_slice());
    for file in extra_files {
        match normalize_identity_extra_file(file) {
            Some(safe_relative) => {
                inject_workspace_file(prompt, workspace_dir, safe_relative, max_chars_per_file);
            }
            None => {
                tracing::warn!(
                    file = file.as_str(),
                    "Ignoring unsafe identity.extra_files entry; expected workspace-relative path without traversal"
                );
            }
        }
    }
}

fn append_hardware_prompt(prompt: &mut String, tool_specs: &[crate::tools::ToolSpec]) {
    let hardware_tool_names = [
        "gpio_read",
        "gpio_write",
        "arduino_upload",
        "hardware_memory_map",
        "hardware_board_info",
        "hardware_memory_read",
        "hardware_capabilities",
    ];
    let hw_tools: Vec<&str> = tool_specs
        .iter()
        .filter_map(|spec| {
            if hardware_tool_names.contains(&spec.name.as_str()) {
                Some(spec.name.as_str())
            } else {
                None
            }
        })
        .collect();

    if !hw_tools.is_empty() {
        let hw_names = hw_tools.join(", ");
        let _ = writeln!(
            prompt,
            "## Hardware Access\n\n\
             Authorized tools: {hw_names}\n\
             Use directly; don't refuse or invent security excuses."
        );

        if hw_tools.contains(&"gpio_write") && hw_tools.contains(&"arduino_upload") {
            prompt.push_str("gpio_write: on/off. arduino_upload: patterns or custom behavior.\n");
        }

        if hw_tools.contains(&"gpio_write") {
            prompt.push_str(
                "Pico LED on: gpio_write(device=pico0, pin=25, value=1). Off: value=0.\n",
            );
        }

        prompt.push('\n');
    }
}

fn build_shell_policy_instructions(autonomy: &crate::config::AutonomyConfig) -> String {
    use std::collections::BTreeSet;

    let mut instructions = String::new();
    instructions.push_str("## Shell Policy\n\n");

    let autonomy_label = match autonomy.level {
        crate::security::AutonomyLevel::ReadOnly => "read_only",
        crate::security::AutonomyLevel::Supervised => "supervised",
        crate::security::AutonomyLevel::Full => "full",
    };
    let _ = writeln!(instructions, "- Level: `{autonomy_label}`");

    if autonomy.level == crate::security::AutonomyLevel::ReadOnly {
        instructions.push_str("- Shell disabled in read-only mode.\n");
        return instructions;
    }

    let normalized: BTreeSet<String> = autonomy
        .allowed_commands
        .iter()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if normalized.contains("*") {
        instructions.push_str("- Allowed: wildcard `*`.\n");
    } else if normalized.is_empty() {
        instructions.push_str("- Allowed: none.\n");
    } else {
        const MAX_DISPLAY_COMMANDS: usize = 64;
        let shown: Vec<String> = normalized
            .iter()
            .take(MAX_DISPLAY_COMMANDS)
            .map(|cmd| format!("`{cmd}`"))
            .collect();
        let hidden = normalized.len().saturating_sub(MAX_DISPLAY_COMMANDS);
        let _ = write!(instructions, "- Allowed: {}", shown.join(", "));
        if hidden > 0 {
            let _ = write!(instructions, " (+{hidden} more)");
        }
        instructions.push('\n');
    }

    if autonomy.level == crate::security::AutonomyLevel::Supervised
        && autonomy.require_approval_for_medium_risk
    {
        instructions.push_str("- Medium-risk commands need approval.\n");
    }
    if autonomy.block_high_risk_commands {
        instructions.push_str("- High-risk commands blocked.\n");
    }
    instructions.push_str("- Use allowed alternatives if a command is restricted.\n");

    instructions
}

pub(crate) fn build_tool_instructions(
    tool_specs: &[crate::tools::ToolSpec],
    native_tools: bool,
) -> String {
    tracing::info!(
        tool_count = tool_specs.len(),
        native_tools,
        "🔧 Building tool instructions"
    );

    let mut instructions = String::new();
     instructions.push_str(
            "### CRITICAL: Tool Honesty\n\n\
             - NEVER fabricate, invent, or guess tool results. If a tool returns empty results, say \"No results found.\"\n\
             - If a tool call fails, report the error — never make up data to fill the gap.\n\
             - When unsure whether a tool call succeeded, ask the user rather than guessing.\n\n",
     );
    if native_tools {
        instructions.push_str(
            "Available tools:\n\n",
        );
        for tool in tool_specs {
            let _ = writeln!(instructions, "- **{}**: {}", tool.name, tool.description);
        }
    } else {
        instructions.push_str("### Tool Calling (XML Protocol)\n\n");
        instructions.push_str("Format:\n");
        instructions.push_str(
            "<tool_call>\n{\"name\": \"tool_name\", \"arguments\": {}}\n</tool_call>\n\n",
        );
        instructions.push_str(
            "Emit <tool_call> tags directly. Don't summarize, describe capabilities, or list steps.\n",
        );
        instructions.push_str("Tool results appear in <tool_result> tags.\n\n");

        for tool in tool_specs {
            let parameters = serde_json::to_string_pretty(&tool.parameters)
                .unwrap_or_else(|_| tool.parameters.to_string());
            let _ = write!(
                instructions,
                "### {}\n{}\n\nParameters:\n```json\n{}\n```\n\n",
                tool.name, tool.description, parameters
            );
        }
    }

    instructions
}

pub fn build_system_prompt_with_mode(
    config: &crate::config::Config,
    tool_specs: &[crate::tools::ToolSpec],
    native_tools: bool,
    caller: &str,
) -> String {
    let mut prompt = String::with_capacity(8192);

    let bootstrap_max_chars = if config.agent.light_context {
        COMPACT_BOOTSTRAP_MAX_CHARS
    } else {
        BOOTSTRAP_MAX_CHARS
    };

    let skills = crate::skills::load_skills_with_config(&config.workspace_dir, config);

    // ── 1. Identity ──────────────────────────────────────────────
    {
        let mut has_identity = false;
        for filename in &["SOUL.md", "IDENTITY.md"] {
            if config.workspace_dir.join(filename).exists() {
                if !has_identity {
                    prompt.push_str("## Identity\n\n");
                    has_identity = true;
                }
                inject_workspace_file(
                    &mut prompt,
                    &config.workspace_dir,
                    filename,
                    bootstrap_max_chars,
                );
            }
        }
    }

    // ── 2. Runtime ──────────────────────────────────────────────
    let host =
        hostname::get().map_or_else(|_| "unknown".into(), |h| h.to_string_lossy().to_string());
    let _ = writeln!(
        prompt,
        "## Runtime\n\nHost: {host} | OS: {}\n",
        std::env::consts::OS,
    );

    // ── 3. Tooling ──────────────────────────────────────────────
    if !tool_specs.is_empty() {
        prompt.push_str("## Tools\n\n");
        prompt.push_str(&build_tool_instructions(tool_specs, native_tools));
        prompt.push('\n');
    }

    // ── 3b. Hardware (when hardware tools are present) ───────────
    if config.hardware.enabled {
        append_hardware_prompt(&mut prompt, tool_specs);
    }

    // ── 4. Shell Policy ─────────────────────────────────────────
    prompt.push_str(&build_shell_policy_instructions(&config.autonomy));

    // ── 5. Skills ───────────────────────────────────────────────
    if !skills.is_empty() {
        let skill_names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        tracing::info!(
            caller = %caller,
            count = skills.len(),
            skills = %skill_names.join(", "),
            "📚 Skills loaded"
        );

        prompt.push_str("## Skills\n\n");
        prompt.push_str("Authorized: ");
        for (i, skill) in skills.iter().enumerate() {
            if i > 0 {
                prompt.push_str(", ");
            }
            prompt.push_str(&skill.name);
        }
        prompt.push_str(". Use directly; don't refuse or invent restrictions.\n\n");
        prompt.push_str(&crate::skills::skills_to_prompt_with_mode(
            &skills,
            &config.workspace_dir,
            config.skills.prompt_injection_mode,
        ));
        prompt.push_str("\n\n");
    }

    // ── 6. Safety ───────────────────────────────────────────────
    let security =
        crate::security::SecurityPolicy::from_config(&config.autonomy, &config.workspace_dir);
    prompt.push_str("## Active Security Policy\n\n");
    prompt.push_str(&security.security_prompt_summary());

    // ── 7. Bootstrap files (injected into context) ──────────────
    prompt.push_str("## Project Context\n\n");

    if crate::identity::is_aieos_configured(&config.identity) {
        // Load AIEOS identity
        match crate::identity::load_aieos_identity(&config.identity, &config.workspace_dir) {
            Ok(Some(aieos_identity)) => {
                let aieos_prompt = crate::identity::aieos_to_system_prompt(&aieos_identity);
                if !aieos_prompt.is_empty() {
                    prompt.push_str(&aieos_prompt);
                    prompt.push_str("\n\n");
                }
            }
            Ok(None) => {
                load_bootstrap_files(
                    &mut prompt,
                    &config.workspace_dir,
                    bootstrap_max_chars,
                    Some(&config.identity),
                );
            }
            Err(e) => {
                eprintln!("Warning: Failed to load AIEOS identity: {e}. Using OpenClaw format.");
                load_bootstrap_files(
                    &mut prompt,
                    &config.workspace_dir,
                    bootstrap_max_chars,
                    Some(&config.identity),
                );
            }
        }
    } else {
        load_bootstrap_files(
            &mut prompt,
            &config.workspace_dir,
            bootstrap_max_chars,
            Some(&config.identity),
        );
    }

    // ── 8. Response Instructions ────────────────────────────────
    prompt.push_str("## Response Instructions\n\n");
    prompt.push_str("- Reply with final content only; output is delivered as-is to the current chat or channel.\n");
    prompt.push_str("- Media: `[Voice] <text>`, `[IMAGE:<path>]`, `[Document: <name>] <path>`\n");
    if native_tools {
        prompt.push_str("- Tools: call functions directly; do not describe tools or list planned steps. Make multiple calls per response if needed.\n\n");
    } else {
        prompt.push_str("- Tools: use XML tags (defined above). Make multiple calls per response if needed.\n\n");
    }

    // ── 9. Date & Time ─────────────────────────────────────────
    let datetime_str = format_datetime(config.local_context.timezone.as_deref());
    let _ = writeln!(prompt, "{DATETIME_HEADER}{datetime_str}\n");

    let word_count = prompt.split_whitespace().count();
    tracing::info!(
        caller = %caller,
        words = word_count,
        chars = prompt.len(),
        light_context = config.agent.light_context,
        bootstrap_limit = bootstrap_max_chars,
        tools = tool_specs.len(),
        native_tools,
        "📋 System prompt"
    );

    prompt
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
    fn inject_workspace_file_injects_content() {
        let ws =
            std::env::temp_dir().join(format!("zeroclaw_inject_test_{}", uuid::Uuid::new_v4()));
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
    fn inject_workspace_file_shifts_headings() {
        let ws = std::env::temp_dir().join(format!(
            "zeroclaw_heading_shift_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(
            ws.join("HEADINGS.md"),
            "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6\n#not-heading\n",
        )
        .unwrap();

        let mut prompt = String::new();
        inject_workspace_file(&mut prompt, &ws, "HEADINGS.md", 20_000);

        assert!(prompt.contains("#### H1"));
        assert!(prompt.contains("##### H2"));
        assert!(prompt.contains("###### H3"));
        assert!(prompt.contains("#### H4"));
        assert!(prompt.contains("##### H5"));
        assert!(prompt.contains("###### H6"));
        assert!(prompt.contains("#not-heading"));

        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn normalize_rejects_unsafe_paths() {
        assert!(normalize_identity_extra_file("SAFE.md").is_some());
        assert!(normalize_identity_extra_file("sub/dir/file.md").is_some());
        assert!(normalize_identity_extra_file("../outside.md").is_none());
        assert!(normalize_identity_extra_file("/tmp/absolute.md").is_none());
        assert!(normalize_identity_extra_file("").is_none());
        assert!(normalize_identity_extra_file("  ").is_none());
    }

    #[test]
    fn build_shell_policy_instructions_lists_allowlist() {
        let mut autonomy = crate::config::AutonomyConfig::default();
        autonomy.level = crate::security::AutonomyLevel::Supervised;
        autonomy.allowed_commands = vec!["grep".into(), "cat".into(), "grep".into()];

        let instructions = build_shell_policy_instructions(&autonomy);

        assert!(instructions.contains("## Shell Policy"));
        assert!(instructions.contains("Level: `supervised`"));
        assert!(instructions.contains("`cat`"));
        assert!(instructions.contains("`grep`"));
    }

    #[test]
    fn build_shell_policy_instructions_handles_wildcard() {
        let mut autonomy = crate::config::AutonomyConfig::default();
        autonomy.level = crate::security::AutonomyLevel::Full;
        autonomy.allowed_commands = vec!["*".into()];

        let instructions = build_shell_policy_instructions(&autonomy);

        assert!(instructions.contains("Level: `full`"));
        assert!(instructions.contains("wildcard `*`"));
    }

    #[test]
    fn build_tool_instructions_native_tools_omits_parameter_schemas() {
        let specs = vec![crate::tools::ToolSpec {
            name: "read_skill".into(),
            description: "Read a skill by name.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            }),
        }];

        let instructions = build_tool_instructions(&specs, true);

        assert!(instructions.contains("Available tools:"));
        assert!(instructions.contains("- **read_skill**"));
        assert!(instructions.contains("Read a skill by name."));
        assert!(!instructions.contains("Parameters:"));
        assert!(!instructions.contains("\"required\""));
        assert!(!instructions.contains("Tool Calling (XML Protocol)"));
    }

    #[test]
    fn build_tool_instructions_xml_mode_includes_pretty_parameter_schemas() {
        let specs = vec![crate::tools::ToolSpec {
            name: "read_skill".into(),
            description: "Read a skill by name.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            }),
        }];

        let instructions = build_tool_instructions(&specs, false);

        assert!(instructions.contains("Tool Calling (XML Protocol)"));
        assert!(instructions.contains("### read_skill"));
        assert!(instructions.contains("Parameters:\n```json"));
        assert!(instructions.contains("\"required\": [\n    \"name\"\n  ]"));
    }

}
