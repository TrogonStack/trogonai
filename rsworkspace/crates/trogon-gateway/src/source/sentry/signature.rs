use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    InvalidHex(hex::FromHexError),
    InvalidKey,
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::InvalidHex(_) => f.write_str("invalid hex encoding"),
            SignatureError::InvalidKey => f.write_str("invalid HMAC key"),
            SignatureError::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignatureError::InvalidHex(error) => Some(error),
            SignatureError::InvalidKey => None,
            SignatureError::Mismatch => None,
        }
    }
}

pub fn verify(secret: &str, body: &[u8], signature_header: &str) -> Result<(), SignatureError> {
    let expected = hex::decode(signature_header).map_err(SignatureError::InvalidHex)?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| SignatureError::InvalidKey)?;
    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| SignatureError::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    fn compute_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn error_display_messages() {
        let hex_error = SignatureError::InvalidHex(hex::decode("zz").unwrap_err());
        assert_eq!(hex_error.to_string(), "invalid hex encoding");
        assert!(hex_error.source().is_some());
        assert_eq!(SignatureError::InvalidKey.to_string(), "invalid HMAC key");
        assert!(SignatureError::InvalidKey.source().is_none());
        assert_eq!(SignatureError::Mismatch.to_string(), "signature mismatch");
        assert!(SignatureError::Mismatch.source().is_none());
    }

    #[test]
    fn valid_signature_passes() {
        let sig = compute_sig("test-secret", b"hello world");
        assert!(verify("test-secret", b"hello world", &sig).is_ok());
    }

    #[test]
    fn wrong_secret_fails() {
        let sig = compute_sig("correct-secret", b"body");
        assert!(matches!(
            verify("wrong-secret", b"body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn tampered_body_fails() {
        let sig = compute_sig("secret", b"original body");
        assert!(matches!(
            verify("secret", b"tampered body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn invalid_hex_fails() {
        assert!(matches!(
            verify("secret", b"body", "not-valid-hex!"),
            Err(SignatureError::InvalidHex(_))
        ));
    }

    #[test]
    fn empty_body_with_valid_sig_passes() {
        let sig = compute_sig("secret", b"");
        assert!(verify("secret", b"", &sig).is_ok());
    }
}
