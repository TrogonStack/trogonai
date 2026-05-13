use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

const GITLAB_SIGNING_TOKEN_BYTES: usize = 32;

#[derive(Debug)]
pub enum GitLabSigningTokenError {
    Empty,
    MissingPrefix,
    InvalidBase64(base64::DecodeError),
    InvalidLength { actual: usize },
}

impl fmt::Display for GitLabSigningTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("signing token must not be empty"),
            Self::MissingPrefix => f.write_str("signing token must start with whsec_"),
            Self::InvalidBase64(_) => f.write_str("signing token must be valid base64"),
            Self::InvalidLength { actual } => {
                write!(f, "signing token must decode to 32 bytes, got {actual}")
            }
        }
    }
}

impl std::error::Error for GitLabSigningTokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidBase64(error) => Some(error),
            Self::Empty | Self::MissingPrefix | Self::InvalidLength { .. } => None,
        }
    }
}

#[derive(Clone)]
pub struct GitLabSigningToken([u8; GITLAB_SIGNING_TOKEN_BYTES]);

impl GitLabSigningToken {
    pub fn new(token: impl AsRef<str>) -> Result<Self, GitLabSigningTokenError> {
        let token = token.as_ref();
        if token.is_empty() {
            return Err(GitLabSigningTokenError::Empty);
        }
        let encoded = token
            .strip_prefix("whsec_")
            .ok_or(GitLabSigningTokenError::MissingPrefix)?;
        if encoded.is_empty() {
            return Err(GitLabSigningTokenError::Empty);
        }
        let decoded = STANDARD
            .decode(encoded)
            .map_err(GitLabSigningTokenError::InvalidBase64)?;
        if decoded.len() != GITLAB_SIGNING_TOKEN_BYTES {
            return Err(GitLabSigningTokenError::InvalidLength { actual: decoded.len() });
        }
        let bytes = decoded.try_into().expect("decoded signing token length was checked");
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for GitLabSigningToken {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl fmt::Debug for GitLabSigningToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GitLabSigningToken(****)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    fn valid_signing_token() -> &'static str {
        "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
    }

    #[test]
    fn gitlab_signing_token_decodes_key() {
        let token = GitLabSigningToken::new(valid_signing_token()).unwrap();
        assert_eq!(token.as_bytes(), b"01234567890123456789012345678901");
        assert_eq!(token.as_ref(), b"01234567890123456789012345678901");
    }

    #[test]
    fn gitlab_signing_token_rejects_bare_key() {
        let err = GitLabSigningToken::new("MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=").unwrap_err();
        assert_eq!(err.to_string(), "signing token must start with whsec_");
        assert!(err.source().is_none());
    }

    #[test]
    fn gitlab_signing_token_rejects_empty_prefixed_key() {
        let err = GitLabSigningToken::new("whsec_").unwrap_err();
        assert_eq!(err.to_string(), "signing token must not be empty");
        assert!(err.source().is_none());
    }

    #[test]
    fn gitlab_signing_token_rejects_invalid_base64() {
        let err = GitLabSigningToken::new("whsec_not-base64!").unwrap_err();
        assert_eq!(err.to_string(), "signing token must be valid base64");
        assert!(err.source().is_some());
    }

    #[test]
    fn gitlab_signing_token_rejects_wrong_key_length() {
        let err = GitLabSigningToken::new("whsec_dGVzdA==").unwrap_err();
        assert_eq!(err.to_string(), "signing token must decode to 32 bytes, got 4");
        assert!(err.source().is_none());
    }

    #[test]
    fn gitlab_signing_token_debug_redacts() {
        let token = GitLabSigningToken::new(valid_signing_token()).unwrap();
        assert_eq!(format!("{token:?}"), "GitLabSigningToken(****)");
    }
}
