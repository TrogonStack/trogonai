//! Token value objects for API key proxying.
//!
//! Token format: `tok_{provider}_{env}_{id}`
//! Example:      `tok_anthropic_prod_a1b2c3`

use std::fmt;

/// The AI provider this token is scoped to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    Anthropic,
    OpenAi,
    Gemini,
    Other(String),
}

impl AiProvider {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl fmt::Display for AiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Deployment environment a token belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Env {
    Prod,
    Staging,
    Dev,
    Test,
    Other(String),
}

impl Env {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Prod => "prod",
            Self::Staging => "staging",
            Self::Dev => "dev",
            Self::Test => "test",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl fmt::Display for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors produced when parsing or constructing an [`ApiKeyToken`].
#[derive(Debug, Clone, PartialEq)]
pub enum TokenError {
    /// Token must start with `tok_`.
    MissingPrefix,
    /// Token has fewer than four `_`-separated segments.
    TooFewSegments,
    /// Provider segment is empty.
    EmptyProvider,
    /// Environment segment is empty.
    EmptyEnv,
    /// Unique ID segment is empty.
    EmptyId,
    /// ID segment contains characters outside `[a-zA-Z0-9]`.
    InvalidIdCharacter(char),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => write!(f, "token must start with 'tok_'"),
            Self::TooFewSegments => {
                write!(f, "token must have format tok_{{provider}}_{{env}}_{{id}}")
            }
            Self::EmptyProvider => write!(f, "provider segment must not be empty"),
            Self::EmptyEnv => write!(f, "env segment must not be empty"),
            Self::EmptyId => write!(f, "id segment must not be empty"),
            Self::InvalidIdCharacter(ch) => {
                write!(f, "id segment contains invalid character: {:?}", ch)
            }
        }
    }
}

impl std::error::Error for TokenError {}

/// A validated proxy token that maps to a real AI-provider API key.
///
/// # Format
///
/// ```text
/// tok_{provider}_{env}_{id}
/// ```
///
/// where `id` is alphanumeric (`[a-zA-Z0-9]+`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ApiKeyToken(String);

impl ApiKeyToken {
    /// Parse and validate a raw token string.
    pub fn new(s: impl Into<String>) -> Result<Self, TokenError> {
        let s: String = s.into();

        if !s.starts_with("tok_") {
            return Err(TokenError::MissingPrefix);
        }

        // Split on '_' with a max of 4 parts: tok, provider, env, id
        // The id part may itself contain underscores — we join the remainder.
        let without_prefix = &s["tok_".len()..];
        let parts: Vec<&str> = without_prefix.splitn(3, '_').collect();

        if parts.len() < 3 {
            return Err(TokenError::TooFewSegments);
        }

        let provider = parts[0];
        let env = parts[1];
        let id = parts[2];

        if provider.is_empty() {
            return Err(TokenError::EmptyProvider);
        }
        if env.is_empty() {
            return Err(TokenError::EmptyEnv);
        }
        if id.is_empty() {
            return Err(TokenError::EmptyId);
        }
        if let Some(ch) = id.chars().find(|c| !c.is_ascii_alphanumeric()) {
            return Err(TokenError::InvalidIdCharacter(ch));
        }

        Ok(Self(s))
    }

    /// Return the raw validated token string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the provider segment (e.g. `"anthropic"`).
    pub fn provider_str(&self) -> &str {
        // Safe: validated in `new`.
        let without_prefix = &self.0["tok_".len()..];
        without_prefix.split('_').next().unwrap_or("")
    }

    /// Return the env segment (e.g. `"prod"`).
    pub fn env_str(&self) -> &str {
        let without_prefix = &self.0["tok_".len()..];
        without_prefix.splitn(3, '_').nth(1).unwrap_or("")
    }
}

impl fmt::Display for ApiKeyToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ApiKeyToken {
    type Error = TokenError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for ApiKeyToken {
    type Error = TokenError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_token_parses() {
        let t = ApiKeyToken::new("tok_anthropic_prod_a1b2c3").unwrap();
        assert_eq!(t.as_str(), "tok_anthropic_prod_a1b2c3");
        assert_eq!(t.provider_str(), "anthropic");
        assert_eq!(t.env_str(), "prod");
    }

    #[test]
    fn valid_token_openai() {
        let t = ApiKeyToken::new("tok_openai_staging_XYZ789").unwrap();
        assert_eq!(t.provider_str(), "openai");
        assert_eq!(t.env_str(), "staging");
    }

    #[test]
    fn missing_prefix_errors() {
        assert_eq!(
            ApiKeyToken::new("openai_prod_abc").unwrap_err(),
            TokenError::MissingPrefix
        );
    }

    #[test]
    fn too_few_segments() {
        assert_eq!(
            ApiKeyToken::new("tok_anthropic").unwrap_err(),
            TokenError::TooFewSegments
        );
        assert_eq!(
            ApiKeyToken::new("tok_anthropic_prod").unwrap_err(),
            TokenError::TooFewSegments
        );
    }

    #[test]
    fn empty_provider() {
        assert_eq!(
            ApiKeyToken::new("tok__prod_abc").unwrap_err(),
            TokenError::EmptyProvider
        );
    }

    #[test]
    fn empty_env() {
        assert_eq!(
            ApiKeyToken::new("tok_anthropic__abc").unwrap_err(),
            TokenError::EmptyEnv
        );
    }

    #[test]
    fn empty_id() {
        assert_eq!(
            ApiKeyToken::new("tok_anthropic_prod_").unwrap_err(),
            TokenError::EmptyId
        );
    }

    #[test]
    fn invalid_id_character() {
        assert!(matches!(
            ApiKeyToken::new("tok_anthropic_prod_abc-def").unwrap_err(),
            TokenError::InvalidIdCharacter('-')
        ));
    }

    /// An underscore in the id segment is not alphanumeric and must be rejected.
    ///
    /// `tok_anthropic_prod_a_b_c` — after splitn(3) the id is `"a_b_c"`;
    /// the underscore fails the `[a-zA-Z0-9]+` check.
    #[test]
    fn underscore_in_id_is_invalid() {
        assert!(matches!(
            ApiKeyToken::new("tok_anthropic_prod_a_b_c").unwrap_err(),
            TokenError::InvalidIdCharacter('_')
        ));
    }

    #[test]
    fn try_from_str() {
        let t = ApiKeyToken::try_from("tok_anthropic_prod_abc123").unwrap();
        assert_eq!(t.as_str(), "tok_anthropic_prod_abc123");
    }

    #[test]
    fn display() {
        let t = ApiKeyToken::new("tok_gemini_dev_zz99").unwrap();
        assert_eq!(t.to_string(), "tok_gemini_dev_zz99");
    }

    #[test]
    fn ai_provider_display() {
        assert_eq!(AiProvider::Anthropic.to_string(), "anthropic");
        assert_eq!(AiProvider::OpenAi.to_string(), "openai");
        assert_eq!(AiProvider::Other("custom".to_string()).to_string(), "custom");
    }

    #[test]
    fn env_display() {
        assert_eq!(Env::Prod.to_string(), "prod");
        assert_eq!(Env::Staging.to_string(), "staging");
        assert_eq!(Env::Other("uat".to_string()).to_string(), "uat");
    }

    #[test]
    fn token_error_display() {
        assert!(TokenError::MissingPrefix.to_string().contains("tok_"));
        assert!(TokenError::TooFewSegments.to_string().contains("format"));
        assert!(TokenError::EmptyId.to_string().contains("id segment"));
        assert!(
            TokenError::InvalidIdCharacter('-')
                .to_string()
                .contains("'-'")
        );
    }
}
