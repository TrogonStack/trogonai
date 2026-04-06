use std::fmt;

use subtle::ConstantTimeEq;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    Missing,
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::Missing => f.write_str("missing secret token header"),
            SignatureError::Mismatch => f.write_str("secret token mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {}

/// Verifies the Telegram webhook secret token using constant-time comparison.
///
/// Telegram sends `X-Telegram-Bot-Api-Secret-Token: <token>` as a plain string.
/// This is compared against the configured secret using constant-time equality
/// to prevent timing side-channel attacks.
pub fn verify(secret: &str, token_header: Option<&str>) -> Result<(), SignatureError> {
    let token = token_header.ok_or(SignatureError::Missing)?;

    if secret.as_bytes().ct_eq(token.as_bytes()).into() {
        Ok(())
    } else {
        Err(SignatureError::Mismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        assert_eq!(
            SignatureError::Missing.to_string(),
            "missing secret token header"
        );
        assert_eq!(
            SignatureError::Mismatch.to_string(),
            "secret token mismatch"
        );
    }

    #[test]
    fn valid_token_passes() {
        assert!(verify("my-secret", Some("my-secret")).is_ok());
    }

    #[test]
    fn wrong_token_fails() {
        assert!(matches!(
            verify("correct-secret", Some("wrong-secret")),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn missing_token_fails() {
        assert!(matches!(
            verify("secret", None),
            Err(SignatureError::Missing)
        ));
    }

    #[test]
    fn empty_secret_matches_empty_token() {
        assert!(verify("", Some("")).is_ok());
    }

    #[test]
    fn empty_secret_does_not_match_nonempty_token() {
        assert!(matches!(
            verify("", Some("something")),
            Err(SignatureError::Mismatch)
        ));
    }
}
