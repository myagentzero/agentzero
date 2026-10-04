use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod audit;
#[cfg(feature = "skill-creation")]
pub mod creator;
pub mod usage_tracker;

/// A skill is a user-defined or community-built capability.
/// Skills live in `~/.agentzero/workspace/skills/<name>/SKILL.md`
/// and can include tool definitions, prompts, and automation scripts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub version: String,
    #[serde(default)]
    pub tools: Vec<SkillTool>,
    #[serde(default)]
    pub prompts: Vec<String>,
    #[serde(skip)]
    pub location: Option<PathBuf>,
}

/// A tool defined by a skill (shell command, HTTP call, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTool {
    pub name: String,
    pub description: String,
    /// "shell", "http", "script"
    pub kind: String,
    /// The command/URL/script to execute
    pub command: String,
    #[serde(default)]
    pub args: HashMap<String, String>,
}

/// Skill manifest parsed from SKILL.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillManifest {
    skill: SkillMeta,
    #[serde(default)]
    tools: Vec<SkillTool>,
    #[serde(default)]
    prompts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillMeta {
    name: String,
    description: String,
    #[serde(default = "default_version")]
    version: String,
}

#[derive(Debug, Clone, Default)]
struct SkillMarkdownMeta {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// Emit a user-visible warning when a skill directory is skipped due to audit
/// findings. When the findings mention blocked scripts and `allow_scripts` is
/// `false`, the message includes actionable remediation guidance so users know
/// how to enable their skill.
fn warn_skipped_skill(path: &Path, summary: &str, allow_scripts: bool) {
    let scripts_blocked = summary.contains("script-like files are blocked");
    if scripts_blocked && !allow_scripts {
        tracing::warn!(
            "skipping skill directory {}: {summary}. \
             To allow script files in skills, set `skills.allow_scripts = true` in your config.",
            path.display(),
        );
        eprintln!(
            "warning: skill '{}' was skipped because it contains script files. \
             Set `skills.allow_scripts = true` in your agentzero config to enable it.",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        );
    } else {
        tracing::warn!(
            "skipping insecure skill directory {}: {summary}",
            path.display(),
        );
    }
}

/// Load skills using runtime config values (preferred at runtime).
pub fn load_skills_with_config(workspace_dir: &Path, config: &crate::config::Config) -> Vec<Skill> {
    load_workspace_skills(workspace_dir, config.skills.allow_scripts)
}

fn load_workspace_skills(workspace_dir: &Path, allow_scripts: bool) -> Vec<Skill> {
    let skills_dir = workspace_dir.join("skills");
    load_skills_from_directory(&skills_dir, allow_scripts)
}

pub fn load_skills_from_directory(skills_dir: &Path, allow_scripts: bool) -> Vec<Skill> {
    if !skills_dir.exists() {
        return Vec::new();
    }

    let mut skills = Vec::new();

    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return skills;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        match audit::audit_skill_directory_with_options(
            &path,
            audit::SkillAuditOptions { allow_scripts },
        ) {
            Ok(report) if report.is_clean() => {}
            Ok(report) => {
                let summary = report.summary();
                warn_skipped_skill(&path, &summary, allow_scripts);
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    "skipping unauditable skill directory {}: {err}",
                    path.display()
                );
                continue;
            }
        }

        // Try SKILL.toml first, then SKILL.md
        let manifest_path = path.join("SKILL.toml");
        let md_path = path.join("SKILL.md");

        if manifest_path.exists() {
            if let Ok(skill) = load_skill_toml(&manifest_path) {
                skills.push(skill);
            }
        } else if md_path.exists() {
            if let Ok(skill) = load_skill_md(&md_path, &path) {
                skills.push(skill);
            }
        }
    }

    skills
}

/// Load a skill from a SKILL.toml manifest
fn load_skill_toml(path: &Path) -> Result<Skill> {
    let content = std::fs::read_to_string(path)?;
    let manifest: SkillManifest = toml::from_str(&content)?;

    Ok(Skill {
        name: manifest.skill.name,
        description: manifest.skill.description,
        version: manifest.skill.version,
        tools: manifest.tools,
        prompts: manifest.prompts,
        location: Some(path.to_path_buf()),
    })
}

/// Load a skill from a SKILL.md file (simpler format)
fn load_skill_md(path: &Path, dir: &Path) -> Result<Skill> {
    let content = std::fs::read_to_string(path)?;
    let parsed = parse_skill_markdown(&content);
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(Skill {
        name: parsed.meta.name.unwrap_or(name),
        description: parsed
            .meta
            .description
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| extract_description(&parsed.body)),
        version: parsed.meta.version.unwrap_or_else(default_version),
        tools: Vec::new(),
        prompts: vec![parsed.body],
        location: Some(path.to_path_buf()),
    })
}

struct ParsedSkillMarkdown {
    meta: SkillMarkdownMeta,
    body: String,
}

fn parse_skill_markdown(content: &str) -> ParsedSkillMarkdown {
    if let Some((frontmatter, body)) = split_skill_frontmatter(content) {
        let meta = parse_simple_frontmatter(&frontmatter);
        return ParsedSkillMarkdown { meta, body };
    }

    ParsedSkillMarkdown {
        meta: SkillMarkdownMeta::default(),
        body: content.to_string(),
    }
}

/// Lightweight YAML-like frontmatter parser for simple `key: value` pairs.
/// Replaces `serde_yaml` to avoid pulling in the full YAML parser (~30KB)
/// for a struct with only 3 optional string fields.
///
/// Only `name`, `description`, and `version` are recognized, matching the
/// standard SKILL.md frontmatter fields. Any other keys are ignored.
fn parse_simple_frontmatter(s: &str) -> SkillMarkdownMeta {
    let mut meta = SkillMarkdownMeta::default();
    for line in s.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim().trim_matches('"').trim_matches('\'');
        match key {
            "name" => meta.name = Some(val.to_string()),
            "description" => meta.description = Some(val.to_string()),
            "version" => meta.version = Some(val.to_string()),
            _ => {}
        }
    }
    meta
}

fn split_skill_frontmatter(content: &str) -> Option<(String, String)> {
    let normalized = content.replace("\r\n", "\n");
    let rest = normalized.strip_prefix("---\n")?;
    if let Some(idx) = rest.find("\n---\n") {
        let frontmatter = rest[..idx].to_string();
        let body = rest[idx + 5..].to_string();
        return Some((frontmatter, body));
    }
    if let Some(frontmatter) = rest.strip_suffix("\n---") {
        return Some((frontmatter.to_string(), String::new()));
    }
    None
}

fn extract_description(content: &str) -> String {
    content
        .lines()
        .find(|line| !line.starts_with('#') && !line.trim().is_empty())
        .unwrap_or("No description")
        .trim()
        .to_string()
}

fn append_xml_escaped(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

fn write_xml_text_element(out: &mut String, indent: usize, tag: &str, value: &str) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push('<');
    out.push_str(tag);
    out.push('>');
    append_xml_escaped(out, value);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

fn resolve_skill_location(skill: &Skill, workspace_dir: &Path) -> PathBuf {
    skill.location.clone().unwrap_or_else(|| {
        workspace_dir
            .join("skills")
            .join(&skill.name)
            .join("SKILL.md")
    })
}

fn render_skill_location(skill: &Skill, workspace_dir: &Path, prefer_relative: bool) -> String {
    let location = resolve_skill_location(skill, workspace_dir);
    if prefer_relative {
        if let Ok(relative) = location.strip_prefix(workspace_dir) {
            return relative.display().to_string();
        }
    }
    location.display().to_string()
}

/// Build the "Skills" system prompt section with configurable verbosity.
pub fn skills_to_prompt_with_mode(
    skills: &[Skill],
    workspace_dir: &Path,
    mode: crate::config::SkillsPromptInjectionMode,
) -> String {
    use std::fmt::Write;

    if skills.is_empty() {
        return String::new();
    }

    let mut prompt = String::from(
        "## Skills\n\n\
         Use these skills directly when relevant; don't refuse or invent restrictions.\n",
    );
    prompt.push_str(match mode {
        crate::config::SkillsPromptInjectionMode::Full => {
            "Skill instructions are preloaded below. Follow them directly; do not read skill files at runtime unless the user asks.\n"
        }
        crate::config::SkillsPromptInjectionMode::Compact => {
            "Only skill summaries are listed below. Call `read_skill(name)` with the skill's `<name>` to load its full instructions before using it.\n"
        }
    });
    if skills.iter().any(|skill| !skill.tools.is_empty()) {
        prompt.push_str(
            "Skill `<tools>` are not callable by name. Run a tool's `<command>` yourself with the `shell` tool (kind `shell`/`script`) or the `http_request` tool (kind `http`), replacing each `{{arg}}` placeholder.\n",
        );
    }
    prompt.push_str("\n<available_skills>\n");

    for skill in skills {
        let _ = writeln!(prompt, "  <skill>");
        write_xml_text_element(&mut prompt, 4, "name", &skill.name);
        write_xml_text_element(&mut prompt, 4, "description", &skill.description);
        let location = render_skill_location(
            skill,
            workspace_dir,
            matches!(mode, crate::config::SkillsPromptInjectionMode::Compact),
        );
        write_xml_text_element(&mut prompt, 4, "location", &location);

        // In Full mode, inline both instructions and tools.
        // In Compact mode, skip instructions (loaded on demand) but keep tools
        // so the LLM knows which skill tools are available.
        if matches!(mode, crate::config::SkillsPromptInjectionMode::Full)
            && !skill.prompts.is_empty()
        {
            let _ = writeln!(prompt, "    <instructions>");
            for instruction in &skill.prompts {
                write_xml_text_element(&mut prompt, 6, "instruction", instruction);
            }
            let _ = writeln!(prompt, "    </instructions>");
        }

        if !skill.tools.is_empty() {
            let _ = writeln!(prompt, "    <tools>");
            for tool in &skill.tools {
                let _ = writeln!(prompt, "      <tool>");
                write_xml_text_element(&mut prompt, 8, "name", &tool.name);
                write_xml_text_element(&mut prompt, 8, "description", &tool.description);
                write_xml_text_element(&mut prompt, 8, "kind", &tool.kind);
                write_xml_text_element(&mut prompt, 8, "command", &tool.command);
                let mut args: Vec<_> = tool.args.iter().collect();
                args.sort();
                for (name, description) in args {
                    prompt.push_str("        <arg name=\"");
                    append_xml_escaped(&mut prompt, name);
                    prompt.push_str("\">");
                    append_xml_escaped(&mut prompt, description);
                    prompt.push_str("</arg>\n");
                }
                let _ = writeln!(prompt, "      </tool>");
            }
            let _ = writeln!(prompt, "    </tools>");
        }

        let _ = writeln!(prompt, "  </skill>");
    }

    prompt.push_str("</available_skills>");
    prompt
}

/// Get the skills directory path
pub fn skills_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("skills")
}

/// Initialize the skills directory with a README
pub fn init_skills_dir(workspace_dir: &Path) -> Result<()> {
    let dir = skills_dir(workspace_dir);
    std::fs::create_dir_all(&dir)?;

    let readme = dir.join("README.md");
    if !readme.exists() {
        std::fs::write(
            &readme,
            "# AgentZero Skills\n\n\
             Each subdirectory is a skill. Create a `SKILL.toml` or `SKILL.md` file inside.\n\n\
             ## SKILL.toml format\n\n\
             ```toml\n\
             [skill]\n\
             name = \"my-skill\"\n\
             description = \"What this skill does\"\n\
             version = \"0.1.0\"\n\n\
             # Shell tool — runs via sh -c (60s timeout, sandboxed env)\n\
             [[tools]]\n\
             name = \"my_tool\"\n\
             description = \"What this tool does\"\n\
             kind = \"shell\"\n\
             command = \"echo {{message}}\"\n\n\
             [tools.args]\n\
             message = \"The message to echo\"\n\n\
             # HTTP tool — makes a GET request (30s timeout)\n\
             [[tools]]\n\
             name = \"check_status\"\n\
             description = \"Check service health\"\n\
             kind = \"http\"\n\
             command = \"https://api.example.com/health\"\n\
             ```\n\n\
             Tools are registered as `skill_name.tool_name` in the agent's tool registry.\n\
             Use `{{arg_name}}` placeholders in `command`; each `[tools.args]` key becomes a\n\
             required parameter the agent provides at call time.\n\n\
             ## SKILL.md format (simpler)\n\n\
             Just write a markdown file with instructions for the agent.\n\
             YAML frontmatter is supported for `name`, `description`, and `version`.\n\
             The agent will read it and follow the instructions.\n\n\
             ## Testing skills\n\n\
             Add a `TEST.sh` file with test cases: `command | exit_code | output_pattern`\n\n\
             ```\n",
        )?;
    }

    Ok(())
}

fn is_git_source(source: &str) -> bool {
    is_git_scheme_source(source, "https://")
        || is_git_scheme_source(source, "http://")
        || is_git_scheme_source(source, "ssh://")
        || is_git_scheme_source(source, "git://")
        || is_git_scp_source(source)
}

fn is_git_scheme_source(source: &str, scheme: &str) -> bool {
    let Some(rest) = source.strip_prefix(scheme) else {
        return false;
    };
    if rest.is_empty() || rest.starts_with('/') {
        return false;
    }

    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
}

fn is_git_scp_source(source: &str) -> bool {
    // SCP-like syntax accepted by git, e.g. git@host:owner/repo.git
    // Keep this strict enough to avoid treating local paths as git remotes.
    let Some((user_host, remote_path)) = source.split_once(':') else {
        return false;
    };
    if remote_path.is_empty() {
        return false;
    }
    if source.contains("://") {
        return false;
    }

    let Some((user, host)) = user_host.split_once('@') else {
        return false;
    };
    !user.is_empty()
        && !host.is_empty()
        && !user.contains('/')
        && !user.contains('\\')
        && !host.contains('/')
        && !host.contains('\\')
}

fn snapshot_skill_children(skills_path: &Path) -> Result<HashSet<PathBuf>> {
    let mut paths = HashSet::new();
    for entry in std::fs::read_dir(skills_path)? {
        let entry = entry?;
        paths.insert(entry.path());
    }
    Ok(paths)
}

fn detect_newly_installed_directory(
    skills_path: &Path,
    before: &HashSet<PathBuf>,
) -> Result<PathBuf> {
    let mut created = Vec::new();
    for entry in std::fs::read_dir(skills_path)? {
        let entry = entry?;
        let path = entry.path();
        if !before.contains(&path) && path.is_dir() {
            created.push(path);
        }
    }

    match created.len() {
        1 => Ok(created.remove(0)),
        0 => anyhow::bail!(
            "Unable to determine installed skill directory after clone (no new directory found)"
        ),
        _ => anyhow::bail!(
            "Unable to determine installed skill directory after clone (multiple new directories found)"
        ),
    }
}

fn enforce_skill_security_audit(
    skill_path: &Path,
    allow_scripts: bool,
) -> Result<audit::SkillAuditReport> {
    let report = audit::audit_skill_directory_with_options(
        skill_path,
        audit::SkillAuditOptions { allow_scripts },
    )?;
    if report.is_clean() {
        return Ok(report);
    }

    anyhow::bail!("Skill security audit failed: {}", report.summary());
}

fn remove_git_metadata(skill_path: &Path) -> Result<()> {
    let git_dir = skill_path.join(".git");
    if git_dir.exists() {
        std::fs::remove_dir_all(&git_dir)
            .with_context(|| format!("failed to remove {}", git_dir.display()))?;
    }
    Ok(())
}

fn copy_dir_recursive_secure(src: &Path, dest: &Path) -> Result<()> {
    let src_meta = std::fs::symlink_metadata(src)
        .with_context(|| format!("failed to read metadata for {}", src.display()))?;
    if src_meta.file_type().is_symlink() {
        anyhow::bail!(
            "Refusing to copy symlinked skill source path: {}",
            src.display()
        );
    }
    if !src_meta.is_dir() {
        anyhow::bail!("Skill source must be a directory: {}", src.display());
    }

    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create destination {}", dest.display()))?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&src_path)
            .with_context(|| format!("failed to read metadata for {}", src_path.display()))?;

        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "Refusing to copy symlink within skill source: {}",
                src_path.display()
            );
        }

        if metadata.is_dir() {
            copy_dir_recursive_secure(&src_path, &dest_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&src_path, &dest_path).with_context(|| {
                format!(
                    "failed to copy skill file from {} to {}",
                    src_path.display(),
                    dest_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn install_local_skill_source(
    source: &str,
    skills_path: &Path,
    allow_scripts: bool,
) -> Result<(PathBuf, usize)> {
    let source_path = PathBuf::from(source);
    if !source_path.exists() {
        anyhow::bail!("Source path does not exist: {source}");
    }

    let source_path = source_path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize source path {source}"))?;
    let _ = enforce_skill_security_audit(&source_path, allow_scripts)?;

    let name = source_path
        .file_name()
        .context("Source path must include a directory name")?;
    let dest = skills_path.join(name);
    if dest.exists() {
        anyhow::bail!("Destination skill already exists: {}", dest.display());
    }

    if let Err(err) = copy_dir_recursive_secure(&source_path, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(err);
    }

    match enforce_skill_security_audit(&dest, allow_scripts) {
        Ok(report) => Ok((dest, report.files_scanned)),
        Err(err) => {
            let _ = std::fs::remove_dir_all(&dest);
            Err(err)
        }
    }
}

fn install_git_skill_source(
    source: &str,
    skills_path: &Path,
    allow_scripts: bool,
) -> Result<(PathBuf, usize)> {
    let before = snapshot_skill_children(skills_path)?;
    let output = std::process::Command::new("git")
        .args(["clone", "--depth", "1", source])
        .current_dir(skills_path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git clone failed: {stderr}");
    }

    let installed_dir = detect_newly_installed_directory(skills_path, &before)?;
    remove_git_metadata(&installed_dir)?;
    match enforce_skill_security_audit(&installed_dir, allow_scripts) {
        Ok(report) => Ok((installed_dir, report.files_scanned)),
        Err(err) => {
            let _ = std::fs::remove_dir_all(&installed_dir);
            Err(err)
        }
    }
}

/// Handle the `skills` CLI command
#[allow(clippy::too_many_lines)]
pub fn handle_command(command: crate::SkillCommands, config: &crate::config::Config) -> Result<()> {
    let workspace_dir = &config.workspace_dir;
    match command {
        crate::SkillCommands::List => {
            let skills = load_skills_with_config(workspace_dir, config);
            if skills.is_empty() {
                println!("No skills installed.");
                println!();
                println!("  Create one: mkdir -p ~/.agentzero/workspace/skills/my-skill");
                println!(
                    "              echo '# My Skill' > ~/.agentzero/workspace/skills/my-skill/SKILL.md"
                );
                println!();
                println!("  Or install: agentzero skills install <source>");
            } else {
                println!("Installed skills ({}):", skills.len());
                println!();
                for skill in &skills {
                    println!(
                        "  {} {} — {}",
                        console::style(&skill.name).white().bold(),
                        console::style(format!("v{}", skill.version)).dim(),
                        skill.description
                    );
                    if !skill.tools.is_empty() {
                        println!(
                            "    Tools: {}",
                            skill
                                .tools
                                .iter()
                                .map(|t| t.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
            }
            println!();
            Ok(())
        }
        crate::SkillCommands::Audit { source } => {
            let source_path = PathBuf::from(&source);
            let target = if source_path.exists() {
                source_path
            } else {
                skills_dir(workspace_dir).join(&source)
            };

            if !target.exists() {
                anyhow::bail!("Skill source or installed skill not found: {source}");
            }

            let report = audit::audit_skill_directory_with_options(
                &target,
                audit::SkillAuditOptions {
                    allow_scripts: config.skills.allow_scripts,
                },
            )?;
            if report.is_clean() {
                println!(
                    "  {} Skill audit passed for {} ({} files scanned).",
                    console::style("✓").green().bold(),
                    target.display(),
                    report.files_scanned
                );
                return Ok(());
            }

            println!(
                "  {} Skill audit failed for {}",
                console::style("✗").red().bold(),
                target.display()
            );
            for finding in report.findings {
                println!("    - {finding}");
            }
            anyhow::bail!("Skill audit failed.");
        }
        crate::SkillCommands::Install { source } => {
            println!("Installing skill from: {source}");

            let skills_path = skills_dir(workspace_dir);
            std::fs::create_dir_all(&skills_path)?;

            let (installed_dir, files_scanned) = if is_git_source(&source) {
                install_git_skill_source(&source, &skills_path, config.skills.allow_scripts)
                    .with_context(|| format!("failed to install git skill source: {source}"))?
            } else {
                install_local_skill_source(&source, &skills_path, config.skills.allow_scripts)
                    .with_context(|| format!("failed to install local skill source: {source}"))?
            };
            println!(
                "  {} Skill installed and audited: {} ({} files scanned)",
                console::style("✓").green().bold(),
                installed_dir.display(),
                files_scanned
            );

            println!("  Security audit completed successfully.");
            Ok(())
        }
        crate::SkillCommands::Remove { name } => {
            // Reject path traversal attempts
            if name.contains("..") || name.contains('/') || name.contains('\\') {
                anyhow::bail!("Invalid skill name: {name}");
            }

            let skill_path = skills_dir(workspace_dir).join(&name);

            // Verify the resolved path is actually inside the skills directory
            let canonical_skills = skills_dir(workspace_dir)
                .canonicalize()
                .unwrap_or_else(|_| skills_dir(workspace_dir));
            if let Ok(canonical_skill) = skill_path.canonicalize() {
                if !canonical_skill.starts_with(&canonical_skills) {
                    anyhow::bail!("Skill path escapes skills directory: {name}");
                }
            }

            if !skill_path.exists() {
                anyhow::bail!("Skill not found: {name}");
            }

            std::fs::remove_dir_all(&skill_path)?;
            println!(
                "  {} Skill '{}' removed.",
                console::style("✓").green().bold(),
                name
            );
            Ok(())
        } // TODO: Enable when SkillCommands::Test variant is added to the CLI.
          // crate::SkillCommands::Test { name, verbose } => { ... }
    }
}

#[cfg(test)]
#[allow(clippy::similar_names)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_empty_skills_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills = load_workspace_skills(dir.path(), false);
        assert!(skills.is_empty());
    }

    #[test]
    fn load_skill_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
[skill]
name = "test-skill"
description = "A test skill"
version = "1.0.0"

[[tools]]
name = "hello"
description = "Says hello"
kind = "shell"
command = "echo hello"
"#,
        )
        .unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
        assert_eq!(skills[0].tools.len(), 1);
        assert_eq!(skills[0].tools[0].name, "hello");
    }

    #[test]
    fn load_skill_from_md() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("md-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.md"),
            "# My Skill\nThis skill does cool things.\n",
        )
        .unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "md-skill");
        assert!(skills[0].description.contains("cool things"));
    }

    #[test]
    fn load_skill_from_md_frontmatter_uses_metadata_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("md-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Use this skill for PDFs\nversion: 1.2.3\n---\n# PDF Processing Guide\nExtract text carefully.\n",
        )
        .unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        assert_eq!(skills[0].description, "Use this skill for PDFs");
        assert_eq!(skills[0].version, "1.2.3");
        assert!(skills[0].prompts[0].contains("# PDF Processing Guide"));
        assert!(!skills[0].prompts[0].contains("name: pdf"));
    }

    #[test]
    fn load_skill_from_md_frontmatter_ignores_non_standard_fields() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("md-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Use this skill for PDFs\nversion: 1.2.3\nauthor: maintainer\ntags:\n  - docs\n  - pdf\n---\n# PDF Processing Guide\nExtract text carefully.\n",
        )
        .unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        assert_eq!(skills[0].description, "Use this skill for PDFs");
        assert_eq!(skills[0].version, "1.2.3");
    }

    #[test]
    fn skills_to_prompt_empty() {
        let prompt = skills_to_prompt_with_mode(
            &[],
            Path::new("/tmp"),
            crate::config::SkillsPromptInjectionMode::Full,
        );
        assert!(prompt.is_empty());
    }

    #[test]
    fn skills_to_prompt_with_skills() {
        let skills = vec![Skill {
            name: "test".to_string(),
            description: "A test".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![],
            prompts: vec!["Do the thing.".to_string()],
            location: None,
        }];
        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp"),
            crate::config::SkillsPromptInjectionMode::Full,
        );
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>test</name>"));
        assert!(prompt.contains("<instruction>Do the thing.</instruction>"));
    }

    #[test]
    fn skills_to_prompt_compact_mode_omits_instructions_but_keeps_tools() {
        let skills = vec![Skill {
            name: "test".to_string(),
            description: "A test".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![SkillTool {
                name: "run".to_string(),
                description: "Run task".to_string(),
                kind: "shell".to_string(),
                command: "echo hi".to_string(),
                args: HashMap::new(),
            }],
            prompts: vec!["Do the thing.".to_string()],
            location: Some(PathBuf::from("/tmp/workspace/skills/test/SKILL.md")),
        }];
        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp/workspace"),
            crate::config::SkillsPromptInjectionMode::Compact,
        );

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>test</name>"));
        assert!(prompt.contains("<location>skills/test/SKILL.md</location>"));
        assert!(prompt.contains("read_skill(name)"));
        assert!(!prompt.contains("<instructions>"));
        assert!(!prompt.contains("<instruction>Do the thing.</instruction>"));
        // Compact mode should still include tools so the LLM knows about them.
        assert!(prompt.contains("<tools>"));
        assert!(prompt.contains("<name>run</name>"));
        assert!(prompt.contains("<command>echo hi</command>"));
    }

    #[test]
    fn init_skills_creates_readme() {
        let dir = tempfile::tempdir().unwrap();
        init_skills_dir(dir.path()).unwrap();
        assert!(dir.path().join("skills").join("README.md").exists());
    }

    #[test]
    fn init_skills_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        init_skills_dir(dir.path()).unwrap();
        init_skills_dir(dir.path()).unwrap(); // second call should not fail
        assert!(dir.path().join("skills").join("README.md").exists());
    }

    #[test]
    fn load_nonexistent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("nonexistent");
        let skills = load_workspace_skills(&fake, false);
        assert!(skills.is_empty());
    }

    #[test]
    fn load_ignores_files_in_skills_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        // A file, not a directory — should be ignored
        fs::write(skills_dir.join("not-a-skill.txt"), "hello").unwrap();
        let skills = load_workspace_skills(dir.path(), false);
        assert!(skills.is_empty());
    }

    #[test]
    fn load_ignores_dir_without_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let empty_skill = skills_dir.join("empty-skill");
        fs::create_dir_all(&empty_skill).unwrap();
        // Directory exists but no SKILL.toml or SKILL.md
        let skills = load_workspace_skills(dir.path(), false);
        assert!(skills.is_empty());
    }

    #[test]
    fn load_multiple_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        for name in ["alpha", "beta", "gamma"] {
            let skill_dir = skills_dir.join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("# {name}\nSkill {name} description.\n"),
            )
            .unwrap();
        }

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 3);
    }

    #[test]
    fn toml_skill_with_multiple_tools() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("multi-tool");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
[skill]
name = "multi-tool"
description = "Has many tools"
version = "2.0.0"

[[tools]]
name = "build"
description = "Build the project"
kind = "shell"
command = "cargo build"

[[tools]]
name = "test"
description = "Run tests"
kind = "shell"
command = "cargo test"

[[tools]]
name = "deploy"
description = "Deploy via HTTP"
kind = "http"
command = "https://api.example.com/deploy"
"#,
        )
        .unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        let s = &skills[0];
        assert_eq!(s.name, "multi-tool");
        assert_eq!(s.version, "2.0.0");
        assert_eq!(s.tools.len(), 3);
        assert_eq!(s.tools[0].name, "build");
        assert_eq!(s.tools[1].kind, "shell");
        assert_eq!(s.tools[2].kind, "http");
    }

    #[test]
    fn toml_skill_minimal() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("minimal");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
[skill]
name = "minimal"
description = "Bare minimum"
"#,
        )
        .unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].version, "0.1.0"); // default version
        assert!(skills[0].tools.is_empty());
    }

    #[test]
    fn toml_skill_invalid_syntax_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("broken");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(skill_dir.join("SKILL.toml"), "this is not valid toml {{{{").unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert!(skills.is_empty()); // broken skill is skipped
    }

    #[test]
    fn md_skill_heading_only() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("heading-only");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(skill_dir.join("SKILL.md"), "# Just a Heading\n").unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "No description");
    }

    #[test]
    fn skills_to_prompt_includes_tools() {
        let skills = vec![Skill {
            name: "weather".to_string(),
            description: "Get weather".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![SkillTool {
                name: "get_weather".to_string(),
                description: "Fetch forecast".to_string(),
                kind: "shell".to_string(),
                command: "curl wttr.in/{{city}}".to_string(),
                args: HashMap::from([("city".to_string(), "City name".to_string())]),
            }],
            prompts: vec![],
            location: None,
        }];
        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp"),
            crate::config::SkillsPromptInjectionMode::Full,
        );
        assert!(prompt.contains("weather"));
        // Skill tools are not registered as callable tools, so the prompt must
        // not present them as such and must give the model the command to run.
        assert!(!prompt.contains("<callable_tools"));
        assert!(prompt.contains("not callable by name"));
        assert!(prompt.contains("<name>get_weather</name>"));
        assert!(prompt.contains("<description>Fetch forecast</description>"));
        assert!(prompt.contains("<kind>shell</kind>"));
        assert!(prompt.contains("<command>curl wttr.in/{{city}}</command>"));
        assert!(prompt.contains("<arg name=\"city\">City name</arg>"));
    }

    #[test]
    fn skills_to_prompt_escapes_xml_content() {
        let skills = vec![Skill {
            name: "xml<skill>".to_string(),
            description: "A & B".to_string(),
            version: "1.0.0".to_string(),
            tools: vec![],
            prompts: vec!["Use <tool> & check \"quotes\".".to_string()],
            location: None,
        }];

        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp"),
            crate::config::SkillsPromptInjectionMode::Full,
        );
        assert!(prompt.contains("<name>xml&lt;skill&gt;</name>"));
        assert!(prompt.contains("<description>A &amp; B</description>"));
        assert!(prompt.contains(
            "<instruction>Use &lt;tool&gt; &amp; check &quot;quotes&quot;.</instruction>"
        ));
    }

    #[test]
    fn git_source_detection_accepts_remote_protocols_and_scp_style() {
        let sources = [
            "https://github.com/some-org/some-skill.git",
            "http://github.com/some-org/some-skill.git",
            "ssh://git@github.com/some-org/some-skill.git",
            "git://github.com/some-org/some-skill.git",
            "git@github.com:some-org/some-skill.git",
            "git@localhost:skills/some-skill.git",
        ];

        for source in sources {
            assert!(
                is_git_source(source),
                "expected git source detection for '{source}'"
            );
        }
    }

    #[test]
    fn git_source_detection_rejects_local_paths_and_invalid_inputs() {
        let sources = [
            "./skills/local-skill",
            "/tmp/skills/local-skill",
            "C:\\skills\\local-skill",
            "git@github.com",
            "ssh://",
            "not-a-url",
            "dir/git@github.com:org/repo.git",
        ];

        for source in sources {
            assert!(
                !is_git_source(source),
                "expected local/invalid source detection for '{source}'"
            );
        }
    }

    #[test]
    fn https_urls_detected_as_git() {
        let git_urls = [
            "https://github.com/some-org/some-skill.git",
            "https://gitlab.com/owner/repo",
        ];
        for url in git_urls {
            assert!(is_git_source(url));
        }
    }

    #[test]
    fn skills_dir_path() {
        let base = std::path::Path::new("/home/user/.agentzero");
        let dir = skills_dir(base);
        assert_eq!(dir, PathBuf::from("/home/user/.agentzero/skills"));
    }

    #[test]
    fn toml_prefers_over_md() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("dual");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            "[skill]\nname = \"from-toml\"\ndescription = \"TOML wins\"\n",
        )
        .unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# From MD\nMD description\n").unwrap();

        let skills = load_workspace_skills(dir.path(), false);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "from-toml"); // TOML takes priority
    }

    #[test]
    fn skill_with_scripts_skipped_when_allow_scripts_false() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("obsidian");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
[skill]
name = "obsidian"
description = "Obsidian vault tool"

[[tools]]
name = "search"
description = "Search vault"
kind = "shell"
command = "obsidian search {{query}}"
"#,
        )
        .unwrap();
        fs::write(skill_dir.join("setup.sh"), "#!/bin/bash\necho setup\n").unwrap();

        // With allow_scripts=false (default), skill should be skipped
        let skills = load_skills_from_directory(&skills_dir, false);
        assert!(
            skills.is_empty(),
            "skill with script files should be skipped when allow_scripts=false"
        );

        // With allow_scripts=true, skill should load
        let skills = load_skills_from_directory(&skills_dir, true);
        assert_eq!(skills.len(), 1, "skill should load when allow_scripts=true");
        assert_eq!(skills[0].name, "obsidian");
    }
}

#[cfg(test)]
mod symlink_tests;
