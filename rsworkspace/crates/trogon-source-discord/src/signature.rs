use std::fmt;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

#[derive(Debug)]
#[non_exhaustive]
pub enum SignatureError {
    InvalidPublicKey,
    InvalidSignatureHex(hex::FromHexError),
    InvalidSignatureLength,
    Mismatch,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::InvalidPublicKey => f.write_str("invalid Ed25519 public key"),
            SignatureError::InvalidSignatureHex(_) => f.write_str("invalid hex in signature"),
            SignatureError::InvalidSignatureLength => f.write_str("invalid signature length"),
            SignatureError::Mismatch => f.write_str("signature mismatch"),
        }
    }
}

impl std::error::Error for SignatureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignatureError::InvalidSignatureHex(e) => Some(e),
            _ => None,
        }
    }
}

/// Verifies a Discord interaction signature using Ed25519.
///
/// Discord sends `X-Signature-Ed25519` (hex-encoded 64-byte signature) and
/// `X-Signature-Timestamp`. The signed message is `timestamp + body`.
pub fn verify(
    public_key: &VerifyingKey,
    timestamp: &str,
    body: &[u8],
    signature_hex: &str,
) -> Result<(), SignatureError> {
    let sig_bytes = hex::decode(signature_hex).map_err(SignatureError::InvalidSignatureHex)?;

    let signature =
        Signature::from_slice(&sig_bytes).map_err(|_| SignatureError::InvalidSignatureLength)?;

    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);

    public_key
        .verify(&message, &signature)
        .map_err(|_| SignatureError::Mismatch)
}

/// Parses a hex-encoded Ed25519 public key from the Discord application settings.
pub fn parse_public_key(hex_key: &str) -> Result<VerifyingKey, SignatureError> {
    let key_bytes = hex::decode(hex_key).map_err(SignatureError::InvalidSignatureHex)?;

    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| SignatureError::InvalidPublicKey)?;

    VerifyingKey::from_bytes(&key_array).map_err(|_| SignatureError::InvalidPublicKey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::error::Error;

    fn test_keypair() -> (SigningKey, VerifyingKey) {
        let signing = SigningKey::from_bytes(&[1u8; 32]);
        let verifying = signing.verifying_key();
        (signing, verifying)
    }

    fn sign(signing_key: &SigningKey, timestamp: &str, body: &[u8]) -> String {
        let mut message = Vec::with_capacity(timestamp.len() + body.len());
        message.extend_from_slice(timestamp.as_bytes());
        message.extend_from_slice(body);
        hex::encode(signing_key.sign(&message).to_bytes())
    }

    #[test]
    fn valid_signature_passes() {
        let (sk, vk) = test_keypair();
        let sig = sign(&sk, "1234567890", b"hello world");
        assert!(verify(&vk, "1234567890", b"hello world", &sig).is_ok());
    }

    #[test]
    fn wrong_timestamp_fails() {
        let (sk, vk) = test_keypair();
        let sig = sign(&sk, "1234567890", b"body");
        assert!(matches!(
            verify(&vk, "9999999999", b"body", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn tampered_body_fails() {
        let (sk, vk) = test_keypair();
        let sig = sign(&sk, "1234567890", b"original");
        assert!(matches!(
            verify(&vk, "1234567890", b"tampered", &sig),
            Err(SignatureError::Mismatch)
        ));
    }

    #[test]
    fn invalid_hex_fails() {
        let (_, vk) = test_keypair();
        assert!(matches!(
            verify(&vk, "1234567890", b"body", "not-valid-hex!"),
            Err(SignatureError::InvalidSignatureHex(_))
        ));
    }

    #[test]
    fn wrong_length_signature_fails() {
        let (_, vk) = test_keypair();
        assert!(matches!(
            verify(&vk, "1234567890", b"body", "deadbeef"),
            Err(SignatureError::InvalidSignatureLength)
        ));
    }

    #[test]
    fn empty_body_with_valid_sig_passes() {
        let (sk, vk) = test_keypair();
        let sig = sign(&sk, "1234567890", b"");
        assert!(verify(&vk, "1234567890", b"", &sig).is_ok());
    }

    #[test]
    fn parse_public_key_valid() {
        let (_, vk) = test_keypair();
        let hex_key = hex::encode(vk.as_bytes());
        let parsed = parse_public_key(&hex_key).unwrap();
        assert_eq!(parsed, vk);
    }

    #[test]
    fn parse_public_key_invalid_hex() {
        assert!(matches!(
            parse_public_key("not-hex!"),
            Err(SignatureError::InvalidSignatureHex(_))
        ));
    }

    #[test]
    fn parse_public_key_wrong_length() {
        assert!(matches!(
            parse_public_key("deadbeef"),
            Err(SignatureError::InvalidPublicKey)
        ));
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(
            SignatureError::InvalidPublicKey.to_string(),
            "invalid Ed25519 public key"
        );
        assert_eq!(
            SignatureError::InvalidSignatureLength.to_string(),
            "invalid signature length"
        );
        assert_eq!(SignatureError::Mismatch.to_string(), "signature mismatch");

        let hex_err = SignatureError::InvalidSignatureHex(hex::decode("zz").unwrap_err());
        assert_eq!(hex_err.to_string(), "invalid hex in signature");
        assert!(hex_err.source().is_some());
        assert!(SignatureError::Mismatch.source().is_none());
    }
}
