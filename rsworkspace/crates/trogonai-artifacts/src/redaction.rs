//! Minimal, high-precision secret detection for artifact previews and exports
//! (§ Security, Secrets and Sanitized Exports).
//!
//! This is a pattern matcher for well-known provider-issued credential formats —
//! deliberately conservative to avoid false positives. It is NOT a full DLP/entropy
//! engine; unknown secret shapes are not caught. Detected tokens are replaced with
//! `[REDACTED:<kind>]` so secrets never appear in previews.

/// `(prefix, kind, min_total_len)` for provider credential tokens. The length
/// guard keeps short coincidental matches (e.g. a bare prefix) from being redacted.
const SECRET_PREFIXES: &[(&str, &str, usize)] = &[
    ("AKIA", "aws_access_key", 20),
    ("ASIA", "aws_temp_key", 20),
    ("ghp_", "github_token", 36),
    ("gho_", "github_oauth_token", 36),
    ("ghu_", "github_user_token", 36),
    ("ghs_", "github_server_token", 36),
    ("github_pat_", "github_pat", 30),
    ("glpat-", "gitlab_pat", 26),
    ("xoxb-", "slack_bot_token", 24),
    ("xoxp-", "slack_user_token", 24),
    ("sk_live_", "stripe_secret_key", 20),
    ("sk_test_", "stripe_test_key", 20),
    ("AIza", "google_api_key", 35),
];

/// Result of redacting secrets from a piece of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionOutcome {
    /// Text with every detected secret replaced by `[REDACTED:<kind>]`.
    pub text: String,
    /// Number of tokens redacted.
    pub occurrences: u32,
    /// Distinct secret kinds detected, in first-seen order.
    pub kinds: Vec<String>,
}

impl RedactionOutcome {
    /// Whether any secret was detected.
    pub fn redacted(&self) -> bool {
        self.occurrences > 0
    }
}

/// Detect and redact known credential tokens in `input`.
pub fn redact_secrets(input: &str) -> RedactionOutcome {
    let mut occurrences = 0u32;
    let mut kinds: Vec<String> = Vec::new();
    let mut out = String::with_capacity(input.len());

    for segment in input.split_inclusive(char::is_whitespace) {
        let token = segment.trim_end_matches(char::is_whitespace);
        let trailing = &segment[token.len()..];
        // The secret may be a bare token or the value of `key=value` / `key:value`.
        let value_start = token.rfind(['=', ':']).map(|idx| idx + 1).unwrap_or(0);
        let (prefix, candidate) = token.split_at(value_start);
        if let Some(kind) = classify_secret(candidate) {
            out.push_str(prefix);
            out.push_str("[REDACTED:");
            out.push_str(kind);
            out.push(']');
            out.push_str(trailing);
            occurrences += 1;
            if !kinds.iter().any(|existing| existing == kind) {
                kinds.push(kind.to_string());
            }
        } else {
            out.push_str(segment);
        }
    }

    RedactionOutcome {
        text: out,
        occurrences,
        kinds,
    }
}

fn classify_secret(token: &str) -> Option<&'static str> {
    if token.starts_with("-----BEGIN") {
        return Some("private_key");
    }
    SECRET_PREFIXES
        .iter()
        .find(|(prefix, _, min_len)| token.len() >= *min_len && token.starts_with(prefix))
        .map(|(_, kind, _)| *kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_credentials_and_preserves_other_text() {
        let input = "deploy with AKIAIOSFODNN7EXAMPLE and token ghp_0123456789abcdef0123456789abcdef0123 done";
        let outcome = redact_secrets(input);
        assert!(outcome.redacted());
        assert_eq!(outcome.occurrences, 2);
        assert!(outcome.text.contains("[REDACTED:aws_access_key]"));
        assert!(outcome.text.contains("[REDACTED:github_token]"));
        assert!(outcome.text.contains("deploy with"));
        assert!(!outcome.text.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(outcome.kinds.contains(&"aws_access_key".to_string()));
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let input = "the quick brown fox jumps over the lazy dog";
        let outcome = redact_secrets(input);
        assert!(!outcome.redacted());
        assert_eq!(outcome.text, input);
        assert_eq!(outcome.occurrences, 0);
    }

    #[test]
    fn short_prefix_lookalikes_are_not_redacted() {
        // Too short to be a real credential; must not be a false positive.
        let outcome = redact_secrets("akia is a name and sk_live_ is a label");
        assert!(!outcome.redacted());
    }
}
