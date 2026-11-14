//! The TypeScript SDK's `redactSecrets` (`Em` in the pinned bundle): the eighteen secret patterns it removes from
//! text that comes from a child's stderr or from a spawn failure before the text is put in an error message.
//!
//! The patterns use lookahead and lookbehind, which the `regex` crate does not have, so they run on
//! [`crate::js_regex`], a matcher with JavaScript's semantics. Python has no such step.

use std::sync::LazyLock;

use crate::js_regex::Regex;

mod constants;

use constants::PATTERNS;

static COMPILED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .filter_map(|(source, ignore_case, replacement)| {
            Regex::new(source, *ignore_case).map(|regex| (regex, *replacement))
        })
        .collect()
});

/// Replaces every secret-shaped token in `text`, as `Em` does.
pub(crate) fn redact(text: &str) -> String {
    COMPILED.iter().fold(text.to_owned(), |text, (regex, replacement)| {
        regex.replace_all(&text, replacement)
    })
}

#[cfg(test)]
mod tests;
