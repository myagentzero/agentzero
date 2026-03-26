//! Utility functions for `ZeroClaw`.
//!
//! This module contains reusable helper functions used across the codebase.

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
pub const ALLOWED_SERIAL_PATH_PREFIXES: &[&str] = &[
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
pub enum MaybeSet<T> {
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
