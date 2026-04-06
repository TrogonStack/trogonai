use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    Missing,
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::Missing => f.write_str("missing token"),
            SignatureError::Mismatch => f.write_str("token mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {}

pub fn verify(secret: &str, provided_token: &str) -> Result<(), SignatureError> {
    if provided_token.is_empty() {
        return Err(SignatureError::Missing);
    }

    let expected = Sha256::digest(secret.as_bytes());
    let provided = Sha256::digest(provided_token.as_bytes());

    let ok = subtle::ConstantTimeEq::ct_eq(expected.as_slice(), provided.as_slice()).unwrap_u8();
    if ok == 1 {
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
        assert_eq!(SignatureError::Missing.to_string(), "missing token");
        assert_eq!(SignatureError::Mismatch.to_string(), "token mismatch");
    }

    #[test]
    fn valid_token_passes() {
        assert!(verify("test-secret", "test-secret").is_ok());
    }

    #[test]
    fn wrong_token_fails() {
        assert!(matches!(
            verify("correct-secret", "wrong-secret"),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn missing_token_fails() {
        assert!(matches!(verify("secret", ""), Err(SignatureError::Missing)));
    }
}
