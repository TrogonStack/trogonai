//! Helpers for NATS subject token safety.
//!
//! See [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names).

/// Returns the first character that is a NATS wildcard (`*`, `>`) or whitespace.
pub fn has_wildcards_or_whitespace(value: &str) -> Option<char> {
    value
        .chars()
        .find(|ch| *ch == '*' || *ch == '>' || ch.is_whitespace())
}

/// True if value has consecutive dots, or starts/ends with a dot.
pub fn has_consecutive_or_boundary_dots(value: &str) -> bool {
    value.contains("..") || value.starts_with('.') || value.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── has_wildcards_or_whitespace ───────────────────────────────────────────

    #[test]
    fn wildcards_clean_string_returns_none() {
        assert_eq!(has_wildcards_or_whitespace("foo.bar.baz"), None);
        assert_eq!(has_wildcards_or_whitespace(""), None);
        assert_eq!(has_wildcards_or_whitespace("a"), None);
    }

    #[test]
    fn wildcards_detects_star() {
        assert_eq!(has_wildcards_or_whitespace("foo.*"), Some('*'));
        assert_eq!(has_wildcards_or_whitespace("*"), Some('*'));
    }

    #[test]
    fn wildcards_detects_gt() {
        assert_eq!(has_wildcards_or_whitespace("foo.>"), Some('>'));
        assert_eq!(has_wildcards_or_whitespace(">"), Some('>'));
    }

    #[test]
    fn wildcards_detects_ascii_space() {
        assert_eq!(has_wildcards_or_whitespace("foo bar"), Some(' '));
    }

    #[test]
    fn wildcards_detects_tab() {
        assert_eq!(has_wildcards_or_whitespace("foo\tbar"), Some('\t'));
    }

    #[test]
    fn wildcards_detects_newline() {
        assert_eq!(has_wildcards_or_whitespace("foo\nbar"), Some('\n'));
    }

    #[test]
    fn wildcards_returns_first_offending_char() {
        // '*' comes before '>' in the string — must return '*'
        assert_eq!(has_wildcards_or_whitespace("*.>"), Some('*'));
    }

    // ── has_consecutive_or_boundary_dots ─────────────────────────────────────

    #[test]
    fn dots_clean_string_returns_false() {
        assert!(!has_consecutive_or_boundary_dots("foo.bar.baz"));
        assert!(!has_consecutive_or_boundary_dots("a"));
        assert!(!has_consecutive_or_boundary_dots(""));
    }

    #[test]
    fn dots_consecutive_dots_returns_true() {
        assert!(has_consecutive_or_boundary_dots("foo..bar"));
        assert!(has_consecutive_or_boundary_dots(".."));
    }

    #[test]
    fn dots_leading_dot_returns_true() {
        assert!(has_consecutive_or_boundary_dots(".foo"));
        assert!(has_consecutive_or_boundary_dots("."));
    }

    #[test]
    fn dots_trailing_dot_returns_true() {
        assert!(has_consecutive_or_boundary_dots("foo."));
    }

    #[test]
    fn dots_single_dot_alone_is_boundary_on_both_sides() {
        // "." starts AND ends with dot
        assert!(has_consecutive_or_boundary_dots("."));
    }
}
