//! AI provider → base URL mapping.

/// Return the HTTPS base URL for a provider name extracted from the request path.
///
/// Returns `None` if the provider is not recognised.
pub fn base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("https://api.anthropic.com"),
        "openai" => Some("https://api.openai.com"),
        "gemini" => Some("https://generativelanguage.googleapis.com"),
        "cohere" => Some("https://api.cohere.ai"),
        "mistral" => Some("https://api.mistral.ai"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_providers() {
        assert_eq!(base_url("anthropic"), Some("https://api.anthropic.com"));
        assert_eq!(base_url("openai"), Some("https://api.openai.com"));
        assert_eq!(
            base_url("gemini"),
            Some("https://generativelanguage.googleapis.com")
        );
        assert_eq!(base_url("cohere"), Some("https://api.cohere.ai"));
        assert_eq!(base_url("mistral"), Some("https://api.mistral.ai"));
    }

    #[test]
    fn unknown_provider_returns_none() {
        assert_eq!(base_url("unknown-provider"), None);
        assert_eq!(base_url(""), None);
    }

    /// Gap 2 (unit): provider matching is case-sensitive.
    /// Uppercase or mixed-case names must return None so the proxy rejects
    /// them with 502 rather than silently routing to the wrong URL.
    #[test]
    fn provider_matching_is_case_sensitive() {
        assert_eq!(base_url("ANTHROPIC"), None);
        assert_eq!(base_url("Anthropic"), None);
        assert_eq!(base_url("OPENAI"), None);
        assert_eq!(base_url("OpenAI"), None);
        assert_eq!(base_url("Gemini"), None);
    }

    // ── Gap 4 ──────────────────────────────────────────────────────────────

    /// Provider matching uses an exact `str` match — no trimming.
    /// A name with leading or trailing whitespace must return `None` so the
    /// proxy rejects it with 502 instead of routing to the wrong URL.
    #[test]
    fn provider_with_leading_or_trailing_whitespace_returns_none() {
        assert_eq!(base_url(" anthropic"), None, "leading space must not match");
        assert_eq!(base_url("anthropic "), None, "trailing space must not match");
        assert_eq!(base_url(" openai "), None, "surrounding spaces must not match");
        assert_eq!(base_url("\tanthropic"), None, "leading tab must not match");
    }

    /// A visually similar provider name using a Unicode homoglyph
    /// (e.g. Greek omicron U+03BF instead of ASCII 'o') must not match.
    #[test]
    fn provider_homoglyph_returns_none() {
        // 'ο' is Greek small letter omicron (U+03BF), not ASCII 'o'.
        assert_eq!(base_url("anthr\u{03BF}pic"), None);
        // Cyrillic 'о' (U+043E) instead of ASCII 'o'.
        assert_eq!(base_url("anthr\u{043E}pic"), None);
    }
}
