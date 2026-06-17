//! Utility functions for `ZeroClaw`.
//!
//! This module contains reusable helper functions used across the codebase.

/// Install the default Rustls TLS backend at process startup.
#[cfg(not(test))]
pub(crate) fn install_default_tls_provider() {
    // Prevents rustls from failing to select a process-level provider when
    // both aws-lc-rs and ring features are available (or neither is selected).
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Warning: Failed to install default TLS provider: {e:?}");
    }
}

#[cfg(test)]
pub(crate) fn install_default_tls_provider() {}

/// Truncate a string to at most `max_chars` characters, appending "..." if truncated.
///
/// This function safely handles multi-byte UTF-8 characters (emoji, CJK, accented characters)
/// by using character boundaries instead of byte indices.
///
/// # Arguments
/// * `s` - The string to truncate
/// * `max_chars` - Maximum number of characters to keep (excluding "...")
///
/// # Returns
/// * Original string if length <= `max_chars`
/// * Truncated string with "..." appended if length > `max_chars`
///
/// # Examples
/// ```ignore
/// use zeroclaw::util::truncate_with_ellipsis;
///
/// // ASCII string - no truncation needed
/// assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
///
/// // ASCII string - truncation needed
/// assert_eq!(truncate_with_ellipsis("hello world", 5), "hello...");
///
/// // Multi-byte UTF-8 (emoji) - safe truncation
/// assert_eq!(truncate_with_ellipsis("Hello 🦀 World", 8), "Hello 🦀...");
/// assert_eq!(truncate_with_ellipsis("😀😀😀😀", 2), "😀😀...");
///
/// // Empty string
/// assert_eq!(truncate_with_ellipsis("", 10), "");
/// ```
pub fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => {
            let truncated = &s[..idx];
            // Trim trailing whitespace for cleaner output
            format!("{}...", truncated.trim_end())
        }
        None => s.to_string(),
    }
}

/// Return the greatest valid UTF-8 char boundary at or below `index`.
///
/// This mirrors `str::floor_char_boundary` behavior while remaining compatible
/// with stable toolchains where that API is not available.
pub fn floor_utf8_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }

    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Allowed serial device path prefixes shared across hardware transports.
const ALLOWED_SERIAL_PATH_PREFIXES: &[&str] = &[
    "/dev/ttyACM",
    "/dev/ttyUSB",
    "/dev/tty.usbmodem",
    "/dev/cu.usbmodem",
    "/dev/tty.usbserial",
    "/dev/cu.usbserial",
    "COM",
];

/// Validate serial device path against per-platform rules.
pub fn is_serial_path_allowed(path: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::sync::OnceLock;
        if !std::path::Path::new(path).is_absolute() {
            return false;
        }
        static PAT: OnceLock<regex::Regex> = OnceLock::new();
        let re = PAT.get_or_init(|| {
            regex::Regex::new(r"^/dev/tty(ACM|USB|S|AMA|MFD)\d+$").expect("valid regex")
        });
        return re.is_match(path);
    }

    #[cfg(target_os = "macos")]
    {
        use std::sync::OnceLock;
        if !std::path::Path::new(path).is_absolute() {
            return false;
        }
        static PAT: OnceLock<regex::Regex> = OnceLock::new();
        let re = PAT.get_or_init(|| {
            regex::Regex::new(r"^/dev/(tty|cu)\.(usbmodem|usbserial)[^\x00/]*$")
                .expect("valid regex")
        });
        return re.is_match(path);
    }

    #[cfg(target_os = "windows")]
    {
        use std::sync::OnceLock;
        static PAT: OnceLock<regex::Regex> = OnceLock::new();
        let re = PAT.get_or_init(|| regex::Regex::new(r"^COM\d{1,3}$").expect("valid regex"));
        return re.is_match(path);
    }

    #[allow(unreachable_code)]
    false
}

/// Strip Unicode format control characters that can cause LLM provider API rejections.
///
/// Removes C1 control characters (U+0080–U+009F, Unicode `Cc`), Unicode General Category
/// `Cf` (format controls), and deprecated tag characters (U+E0001–U+E007F), while preserving
/// normal whitespace (`\n`, `\r`, `\t`), printable text, emoji, and CJK characters.
pub fn strip_unicode_format_controls(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            // Keep normal ASCII whitespace
            if c == '\n' || c == '\r' || c == '\t' {
                return true;
            }
            // Strip non-printable ASCII controls (0x00–0x1F, 0x7F) except the whitespace above
            if c.is_ascii_control() {
                return false;
            }
            // Strip C1 control characters (0x80–0x9F, Unicode Cc) — not caught by is_ascii_control()
            if ('\u{0080}'..='\u{009F}').contains(&c) {
                return false;
            }
            // Strip Unicode General Category Cf (format controls)
            if is_unicode_format_control(c) {
                return false;
            }
            // Strip deprecated tag characters U+E0001–U+E007F
            if ('\u{E0001}'..='\u{E007F}').contains(&c) {
                return false;
            }
            true
        })
        .collect()
}

/// Check if a character is a Unicode format control (General Category Cf).
fn is_unicode_format_control(c: char) -> bool {
    matches!(c,
        '\u{00AD}'           // SOFT HYPHEN
        | '\u{0600}'..='\u{0605}' // Arabic number signs
        | '\u{061C}'         // ARABIC LETTER MARK
        | '\u{06DD}'         // ARABIC END OF AYAH
        | '\u{070F}'         // SYRIAC ABBREVIATION MARK
        | '\u{0890}'..='\u{0891}' // Arabic pound/piastre marks
        | '\u{08E2}'         // ARABIC DISPUTED END OF AYAH
        | '\u{180E}'         // MONGOLIAN VOWEL SEPARATOR
        | '\u{200B}'..='\u{200F}' // ZWSP, ZWNJ, ZWJ, LRM, RLM
        | '\u{202A}'..='\u{202E}' // Bidi embedding/override
        | '\u{2060}'..='\u{2064}' // WJ, function application, etc.
        | '\u{2066}'..='\u{2069}' // Bidi isolates
        | '\u{FEFF}'         // BOM / ZWNBSP
        | '\u{FFF9}'..='\u{FFFB}' // Interlinear annotation anchors
        | '\u{110BD}'        // KAITHI NUMBER SIGN
        | '\u{110CD}'        // KAITHI NUMBER SIGN ABOVE
        | '\u{13430}'..='\u{1343F}' // Egyptian hieroglyph format controls
        | '\u{1BCA0}'..='\u{1BCA3}' // Shorthand format controls
        | '\u{1D173}'..='\u{1D17A}' // Musical symbol format controls
    )
}

/// Utility enum for handling optional values.
enum MaybeSet<T> {
    Set(T),
    Unset,
    Null,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_ascii_no_truncation() {
        // ASCII string shorter than limit - no change
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
        assert_eq!(truncate_with_ellipsis("hello world", 50), "hello world");
    }

    #[test]
    fn test_truncate_ascii_with_truncation() {
        // ASCII string longer than limit - truncates
        assert_eq!(truncate_with_ellipsis("hello world", 5), "hello...");
        assert_eq!(
            truncate_with_ellipsis("This is a long message", 10),
            "This is a..."
        );
    }

    #[test]
    fn test_truncate_empty_string() {
        assert_eq!(truncate_with_ellipsis("", 10), "");
    }

    #[test]
    fn test_truncate_at_exact_boundary() {
        // String exactly at boundary - no truncation
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_emoji_single() {
        // Single emoji (4 bytes) - should not panic
        let s = "🦀";
        assert_eq!(truncate_with_ellipsis(s, 10), s);
        assert_eq!(truncate_with_ellipsis(s, 1), s);
    }

    #[test]
    fn test_truncate_emoji_multiple() {
        // Multiple emoji - safe truncation at character boundary
        let s = "😀😀😀😀"; // 4 emoji, each 4 bytes = 16 bytes total
        assert_eq!(truncate_with_ellipsis(s, 2), "😀😀...");
        assert_eq!(truncate_with_ellipsis(s, 3), "😀😀😀...");
    }

    #[test]
    fn test_truncate_mixed_ascii_emoji() {
        // Mixed ASCII and emoji
        assert_eq!(truncate_with_ellipsis("Hello 🦀 World", 8), "Hello 🦀...");
        assert_eq!(truncate_with_ellipsis("Hi 😊", 10), "Hi 😊");
    }

    #[test]
    fn test_truncate_cjk_characters() {
        // CJK characters (Chinese - each is 3 bytes)
        let s = "这是一个测试消息用来触发崩溃的中文"; // 21 characters
        let result = truncate_with_ellipsis(s, 16);
        assert!(result.ends_with("..."));
        assert!(result.is_char_boundary(result.len() - 1));
    }

    #[test]
    fn test_truncate_accented_characters() {
        // Accented characters (2 bytes each in UTF-8)
        let s = "café résumé naïve";
        assert_eq!(truncate_with_ellipsis(s, 10), "café résum...");
    }

    #[test]
    fn test_truncate_unicode_edge_case() {
        // Mix of 1-byte, 2-byte, 3-byte, and 4-byte characters
        let s = "aé你好🦀"; // 1 + 1 + 2 + 2 + 4 bytes = 10 bytes, 5 chars
        assert_eq!(truncate_with_ellipsis(s, 3), "aé你...");
    }

    #[test]
    fn test_truncate_long_string() {
        // Long ASCII string
        let s = "a".repeat(200);
        let result = truncate_with_ellipsis(&s, 50);
        assert_eq!(result.len(), 53); // 50 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_zero_max_chars() {
        // Edge case: max_chars = 0
        assert_eq!(truncate_with_ellipsis("hello", 0), "...");
    }

    #[test]
    fn test_floor_utf8_char_boundary_ascii() {
        assert_eq!(floor_utf8_char_boundary("hello", 0), 0);
        assert_eq!(floor_utf8_char_boundary("hello", 3), 3);
        assert_eq!(floor_utf8_char_boundary("hello", 99), 5);
    }

    #[test]
    fn test_floor_utf8_char_boundary_multibyte() {
        let s = "aé你🦀";
        assert_eq!(floor_utf8_char_boundary(s, 1), 1);
        // Index 2 is inside "é" (2-byte char), floor should move back to 1.
        assert_eq!(floor_utf8_char_boundary(s, 2), 1);
        // Index 5 is inside "你" (3-byte char), floor should move back to 3.
        assert_eq!(floor_utf8_char_boundary(s, 5), 3);
    }

    #[test]
    fn strip_unicode_format_controls_removes_word_joiner() {
        let input = "hello\u{2060}world";
        assert_eq!(strip_unicode_format_controls(input), "helloworld");
    }

    #[test]
    fn strip_unicode_format_controls_removes_zwsp_and_bom() {
        let input = "\u{FEFF}hello\u{200B}world";
        assert_eq!(strip_unicode_format_controls(input), "helloworld");
    }

    #[test]
    fn strip_unicode_format_controls_removes_bidi_and_soft_hyphen() {
        let input = "ab\u{200E}cd\u{00AD}ef\u{202A}gh";
        assert_eq!(strip_unicode_format_controls(input), "abcdefgh");
    }

    #[test]
    fn strip_unicode_format_controls_removes_tag_characters() {
        let input = "hello\u{E0001}\u{E0041}world";
        assert_eq!(strip_unicode_format_controls(input), "helloworld");
    }

    #[test]
    fn strip_unicode_format_controls_preserves_normal_text() {
        let input = "Hello, World! 🦀 你好 café\nnewline\ttab\r\n";
        assert_eq!(strip_unicode_format_controls(input), input);
    }

    #[test]
    fn strip_unicode_format_controls_empty_string() {
        assert_eq!(strip_unicode_format_controls(""), "");
    }

    #[test]
    fn strip_unicode_format_controls_only_controls() {
        let input = "\u{200B}\u{200C}\u{200D}\u{2060}\u{FEFF}";
        assert_eq!(strip_unicode_format_controls(input), "");
    }

    #[test]
    fn strip_unicode_format_controls_removes_c1_controls() {
        // U+0095 MESSAGE WAITING — the specific character from the API error
        let input = "hello\u{0095}world";
        assert_eq!(strip_unicode_format_controls(input), "helloworld");
        // Full C1 range U+0080–U+009F
        let input2 = "a\u{0080}b\u{008A}c\u{009F}d";
        assert_eq!(strip_unicode_format_controls(input2), "abcd");
    }
}

// ── CLI Tool Discovery ──────────────────────────────────────────────────────

use std::path::PathBuf;

/// Category of a discovered CLI tool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum CliCategory {
    VersionControl,
    Language,
    PackageManager,
    Container,
    Build,
    Cloud,
}

impl std::fmt::Display for CliCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionControl => write!(f, "Version Control"),
            Self::Language => write!(f, "Language"),
            Self::PackageManager => write!(f, "Package Manager"),
            Self::Container => write!(f, "Container"),
            Self::Build => write!(f, "Build"),
            Self::Cloud => write!(f, "Cloud"),
        }
    }
}

/// A discovered CLI tool with metadata.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveredCli {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub category: CliCategory,
}

/// Known CLI tools to scan for.
struct KnownCli {
    name: &'static str,
    version_args: &'static [&'static str],
    category: CliCategory,
}

const KNOWN_CLIS: &[KnownCli] = &[
    KnownCli {
        name: "git",
        version_args: &["--version"],
        category: CliCategory::VersionControl,
    },
    KnownCli {
        name: "python",
        version_args: &["--version"],
        category: CliCategory::Language,
    },
    KnownCli {
        name: "python3",
        version_args: &["--version"],
        category: CliCategory::Language,
    },
    KnownCli {
        name: "node",
        version_args: &["--version"],
        category: CliCategory::Language,
    },
    KnownCli {
        name: "npm",
        version_args: &["--version"],
        category: CliCategory::PackageManager,
    },
    KnownCli {
        name: "pip",
        version_args: &["--version"],
        category: CliCategory::PackageManager,
    },
    KnownCli {
        name: "pip3",
        version_args: &["--version"],
        category: CliCategory::PackageManager,
    },
    KnownCli {
        name: "make",
        version_args: &["--version"],
        category: CliCategory::Build,
    },
];

/// Discover available CLI tools on the system.
/// Scans PATH for known tools and returns metadata for each found.
pub fn discover_cli_tools(additional: &[String], excluded: &[String]) -> Vec<DiscoveredCli> {
    let mut results = Vec::new();

    for known in KNOWN_CLIS {
        if excluded.iter().any(|e| e == known.name) {
            continue;
        }
        if let Some(cli) = probe_cli(known.name, known.version_args, known.category.clone()) {
            results.push(cli);
        }
    }

    // Probe additional user-specified tools
    for tool_name in additional {
        if excluded.iter().any(|e| e == tool_name) {
            continue;
        }
        // Skip if already discovered
        if results.iter().any(|r| r.name == *tool_name) {
            continue;
        }
        if let Some(cli) = probe_cli(tool_name, &["--version"], CliCategory::Build) {
            results.push(cli);
        }
    }

    results
}

/// Probe a single CLI tool: check if it exists and get its version.
fn probe_cli(name: &str, version_args: &[&str], category: CliCategory) -> Option<DiscoveredCli> {
    // Try to find the tool using `which` (Unix) or `where` (Windows)
    let path = find_executable(name)?;

    // Try to get version
    let version = get_version(name, version_args);

    Some(DiscoveredCli {
        name: name.to_string(),
        path,
        version,
        category,
    })
}

/// Find an executable on PATH by walking PATH entries — no subprocess needed.
fn find_executable(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        #[cfg(target_os = "windows")]
        let candidate = {
            let p = dir.join(name);
            if p.extension().is_none() {
                p.with_extension("exe")
            } else {
                p
            }
        };
        #[cfg(not(target_os = "windows"))]
        let candidate = dir.join(name);

        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Get the version string of a CLI tool.
/// Enforces a 2-second timeout to avoid hanging on slow or broken tools.
fn get_version(name: &str, args: &[&str]) -> Option<String> {
    let mut child = std::process::Command::new(name)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }

    let output = child.wait_with_output().ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Some tools print version to stderr (e.g., pip)
    let version_text = if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim().to_string()
    };

    // Extract first line only
    let first_line = version_text.lines().next()?.trim().to_string();
    if first_line.is_empty() {
        None
    } else {
        Some(first_line)
    }
}

#[cfg(test)]
mod cli_discovery_tests {
    use super::*;

    #[test]
    fn discover_returns_vec() {
        // Just verify it runs without panic
        let results = discover_cli_tools(&[], &[]);
        // We can't assert specific tools exist in CI, but structure is valid
        for cli in &results {
            assert!(!cli.name.is_empty());
        }
    }

    #[test]
    fn excluded_tools_are_skipped() {
        let results = discover_cli_tools(&[], &["git".to_string()]);
        assert!(!results.iter().any(|r| r.name == "git"));
    }

    #[test]
    fn category_display() {
        assert_eq!(CliCategory::VersionControl.to_string(), "Version Control");
        assert_eq!(CliCategory::Language.to_string(), "Language");
        assert_eq!(CliCategory::PackageManager.to_string(), "Package Manager");
        assert_eq!(CliCategory::Container.to_string(), "Container");
        assert_eq!(CliCategory::Build.to_string(), "Build");
        assert_eq!(CliCategory::Cloud.to_string(), "Cloud");
    }
}
