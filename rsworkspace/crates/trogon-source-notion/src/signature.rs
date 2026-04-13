use std::fmt;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::NotionVerificationToken;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    MissingPrefix,
    InvalidHex(hex::FromHexError),
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => f.write_str("missing sha256= prefix"),
            Self::InvalidHex(_) => f.write_str("invalid hex encoding"),
            Self::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidHex(err) => Some(err),
            Self::MissingPrefix | Self::Mismatch => None,
        }
    }
}

pub fn verify(
    secret: &NotionVerificationToken,
    body: &[u8],
    signature_header: &str,
) -> Result<(), SignatureError> {
    let hex_signature = signature_header
        .strip_prefix("sha256=")
        .ok_or(SignatureError::MissingPrefix)?;

    let expected = hex::decode(hex_signature).map_err(SignatureError::InvalidHex)?;

    let mut mac = HmacSha256::new_from_slice(secret.as_str().as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(body);
    mac.verify_slice(&expected)
        .map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NotionVerificationToken;
    use std::error::Error;

    fn compute_sig(secret: &NotionVerificationToken, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_str().as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(
            SignatureError::MissingPrefix.to_string(),
            "missing sha256= prefix"
        );
        let hex_err = SignatureError::InvalidHex(hex::decode("zz").unwrap_err());
        assert_eq!(hex_err.to_string(), "invalid hex encoding");
        assert!(hex_err.source().is_some());
        assert_eq!(SignatureError::Mismatch.to_string(), "signature mismatch");
    }

    #[test]
    fn valid_signature_passes() {
        let secret = NotionVerificationToken::new("test-secret").unwrap();
        let sig = compute_sig(&secret, b"hello world");
        assert!(verify(&secret, b"hello world", &sig).is_ok());
    }

    #[test]
    fn wrong_secret_fails() {
        let correct_secret = NotionVerificationToken::new("correct-secret").unwrap();
        let wrong_secret = NotionVerificationToken::new("wrong-secret").unwrap();
        let sig = compute_sig(&correct_secret, b"body");
        assert!(matches!(
            verify(&wrong_secret, b"body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn missing_sha256_prefix_fails() {
        let secret = NotionVerificationToken::new("secret").unwrap();
        let sig = compute_sig(&secret, b"body");
        let raw_hex = sig.strip_prefix("sha256=").unwrap().to_string();
        assert!(matches!(
            verify(&secret, b"body", &raw_hex),
            Err(SignatureError::MissingPrefix)
        ));
    }

    #[test]
    fn invalid_hex_fails() {
        let secret = NotionVerificationToken::new("secret").unwrap();
        assert!(matches!(
            verify(&secret, b"body", "sha256=not-valid-hex!"),
            Err(SignatureError::InvalidHex(_))
        ));
    }
}
