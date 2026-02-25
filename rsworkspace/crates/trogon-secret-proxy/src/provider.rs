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
}
