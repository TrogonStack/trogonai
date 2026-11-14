//! The TypeScript SDK's `redactSecrets` (`Em` in the pinned bundle): the eighteen secret patterns it removes from
//! text that comes from a child's stderr or from a spawn failure before the text is put in an error message.
//!
//! The patterns use lookahead and lookbehind, which the `regex` crate does not have, so they run on
//! [`crate::js_regex`], a matcher with JavaScript's semantics. Python has no such step.

use std::sync::LazyLock;

use crate::js_regex::Regex;

/// `Uet` followed by `Fet` in the pinned bundle, in the order `Em` applies them: the pattern source, whether the
/// `i` flag is set (every pattern also has `g`), and the replacement.
const PATTERNS: [(&str, bool, &str); 18] = [
    (
        r#"\bBearer\s+(?=[A-Za-z0-9._~+/=-](?:[A-Za-z0-9._~+/=-]|%[0-9A-Fa-f]{2}){7})[A-Za-z0-9._~+/=-](?:[A-Za-z0-9._~+/=-]+|%[0-9A-Fa-f]{2})*"#,
        true,
        "Bearer <token>",
    ),
    (r#"(:\s*)Basic\s+[A-Za-z0-9+/=]{8,}"#, true, "$1Basic <token>"),
    (
        r#"(?:\bsk-ant-[A-Za-z0-9_-]{8,}|sk-ant-(?=[A-Za-z0-9_-]{0,64}?(?=[A-Za-z0-9]{8})(?=[A-Za-z0-9]{0,64}[0-9])(?=[A-Za-z0-9]{0,64}[A-Za-z]))[A-Za-z0-9_-]{8,})"#,
        false,
        "<token>",
    ),
    (
        r#"(?:\bsk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{20,}|sk-(?:proj|svcacct|admin)-(?=[A-Za-z0-9_-]{0,64}?(?=[A-Za-z0-9]{20})(?=[A-Za-z0-9]{0,64}[0-9])(?=[A-Za-z0-9]{0,64}[A-Za-z]))[A-Za-z0-9_-]{20,})"#,
        false,
        "<token>",
    ),
    (r#"\bsk-[A-Za-z0-9_-]{20,}"#, false, "<token>"),
    (
        r#"(^|[^A-Za-z0-9+])(?:AKIA|ASIA|ABIA|ACCA|A3T[A-Z0-9])[A-Z0-9]{16}(?![A-Za-z0-9+=])"#,
        false,
        "$1<token>",
    ),
    (
        r#"(?:\bgh[opusr]_[A-Za-z0-9]{20,}|gh[opusr]_(?=[A-Za-z0-9]{0,64}[0-9])[A-Za-z0-9]{20,})"#,
        false,
        "<token>",
    ),
    (r#"github_pat_[A-Za-z0-9_]{82,}"#, false, "<token>"),
    (r#"sq0(?:atp|csp)-[A-Za-z0-9_-]{22,}"#, false, "<token>"),
    (r#"(^|[^A-Za-z0-9+/])EAAA[A-Za-z0-9+/=%_-]{56,}"#, false, "$1<token>"),
    (
        r#"(?:\bgl(?:pat|dt|rt|ft|soat|oas|agent|ptt|cbt|imt|ffct)-[A-Za-z0-9_=-]{20,}(?:\.[0-9a-z]{9})?|gl(?:pat|dt|rt|ft|soat|oas|agent|ptt|cbt|imt|ffct)-(?=[A-Za-z0-9_=-]{0,64}[0-9])[A-Za-z0-9_=-]{20,}(?:\.[0-9a-z]{9})?)"#,
        false,
        "<token>",
    ),
    (
        r#"(?:\b(?:xox[abe-z](?:\.xox[a-z])?|xapp|xwfp)-(?=[A-Za-z0-9+/=%_-]{0,64}[0-9])[A-Za-z0-9+/=%_-]{10,}|(?:xox[abe-z](?:\.xox[a-z])?|xapp|xwfp)-(?=[0-9])[A-Za-z0-9+/=%_-]{10,})"#,
        true,
        "<token>",
    ),
    (
        r#"(?:\bxox[cd]-[A-Za-z0-9+/=%_-]{10,}|xox[cd]-(?=[A-Za-z0-9+/=%_-]{0,64}[0-9])[A-Za-z0-9+/=%_-]{16,})"#,
        true,
        "<token>",
    ),
    (
        r#"hooks\.slack\.com\/(?:services|workflows|triggers)\/[A-Za-z0-9/_-]+"#,
        true,
        "hooks.slack.com/<redacted>",
    ),
    (
        r#"(?<![A-Za-z0-9_-])(?=[A-Za-z0-9_-]+\.[A-Za-z0-9_-]{10}[A-Za-z0-9_-]*\.[A-Za-z0-9_-]{10}[A-Za-z0-9_-]*)((?:[A-Za-z0-9_-]*?-)??)ey[A-Za-z0-9_-]{10}[A-Za-z0-9_-]*\.[A-Za-z0-9_-]{10}[A-Za-z0-9_-]*\.[A-Za-z0-9_-]{10}[A-Za-z0-9_-]*"#,
        false,
        "$1<jwt>",
    ),
    (
        r#"(^|[^A-Za-z0-9+_-])AIza[A-Za-z0-9_-]{35}(?![A-Za-z0-9+=_-])"#,
        false,
        "$1<token>",
    ),
    (
        r#"(^|[^A-Za-z0-9+_-])GOCSPX-[A-Za-z0-9_-]{28}(?![A-Za-z0-9+=_-])"#,
        false,
        "$1<token>",
    ),
    (
        r#"(^|[^A-Za-z0-9+_-])[sr]k_(?:live|test|prod)_(?=[A-Za-z0-9]{0,64}[0-9])[A-Za-z0-9]{24,}(?![A-Za-z0-9+=_-])"#,
        false,
        "$1<token>",
    ),
];

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
