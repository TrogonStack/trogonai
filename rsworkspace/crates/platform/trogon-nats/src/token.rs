//! Helpers for NATS subject token safety.
//!
//! See [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names).

/// Returns the first character that is a NATS wildcard (`*`, `>`), whitespace,
/// or a control character such as NUL.
pub fn first_unsafe_char(value: &str) -> Option<char> {
    value.chars().find(|ch| is_unsafe_char(*ch))
}

/// True for a NATS wildcard (`*`, `>`), whitespace, or a control character.
pub fn is_unsafe_char(ch: char) -> bool {
    ch == '*' || ch == '>' || ch.is_whitespace() || ch.is_control()
}

/// True if value has consecutive dots, or starts/ends with a dot.
pub fn has_consecutive_or_boundary_dots(value: &str) -> bool {
    value.contains("..") || value.starts_with('.') || value.ends_with('.')
}
