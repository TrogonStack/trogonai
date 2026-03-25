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
    fn clean_token_returns_none() {
        assert_eq!(has_wildcards_or_whitespace("valid-token"), None);
    }

    #[test]
    fn asterisk_wildcard_is_detected() {
        assert_eq!(has_wildcards_or_whitespace("tok*en"), Some('*'));
    }

    #[test]
    fn gt_wildcard_is_detected() {
        assert_eq!(has_wildcards_or_whitespace("tok>en"), Some('>'));
    }

    #[test]
    fn leading_gt_is_detected() {
        assert_eq!(has_wildcards_or_whitespace(">"), Some('>'));
    }

    #[test]
    fn space_is_detected() {
        assert_eq!(has_wildcards_or_whitespace("tok en"), Some(' '));
    }

    #[test]
    fn tab_is_detected() {
        assert_eq!(has_wildcards_or_whitespace("tok\ten"), Some('\t'));
    }

    #[test]
    fn newline_is_detected() {
        assert_eq!(has_wildcards_or_whitespace("tok\nen"), Some('\n'));
    }

    #[test]
    fn empty_string_returns_none() {
        assert_eq!(has_wildcards_or_whitespace(""), None);
    }

    // ── has_consecutive_or_boundary_dots ─────────────────────────────────────

    #[test]
    fn single_dot_in_middle_is_valid() {
        assert!(!has_consecutive_or_boundary_dots("a.b"));
    }

    #[test]
    fn multiple_single_dots_in_middle_are_valid() {
        assert!(!has_consecutive_or_boundary_dots("a.b.c.d"));
    }

    #[test]
    fn consecutive_dots_returns_true() {
        assert!(has_consecutive_or_boundary_dots("a..b"));
    }

    #[test]
    fn leading_dot_returns_true() {
        assert!(has_consecutive_or_boundary_dots(".abc"));
    }

    #[test]
    fn trailing_dot_returns_true() {
        assert!(has_consecutive_or_boundary_dots("abc."));
    }

    #[test]
    fn only_dots_returns_true() {
        assert!(has_consecutive_or_boundary_dots(".."));
    }

    #[test]
    fn empty_string_is_clean() {
        assert!(!has_consecutive_or_boundary_dots(""));
    }

    #[test]
    fn clean_token_no_dots_is_clean() {
        assert!(!has_consecutive_or_boundary_dots("nodots"));
    }
}
