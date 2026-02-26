//! Helpers for NATS subject token safety.
//!
//! See [NATS subject naming](https://docs.nats.io/nats-concepts/subjects#characters-allowed-and-recommended-for-subject-names).

/// Returns the first character that is a NATS wildcard (`*`, `>`) or whitespace.
pub(crate) fn has_wildcards_or_whitespace(value: &str) -> Option<char> {
    value
        .chars()
        .find(|ch| *ch == '*' || *ch == '>' || ch.is_whitespace())
}

/// True if value has consecutive dots, or starts/ends with a dot.
pub(crate) fn has_consecutive_or_boundary_dots(value: &str) -> bool {
    value.contains("..") || value.starts_with('.') || value.ends_with('.')
}
