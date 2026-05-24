use std::fmt;

use ed25519_dalek::VerifyingKey;

use super::error::SignatureVerificationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519PublicKey([u8; 32]);

impl Ed25519PublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(raw: &str) -> Result<Self, SignatureVerificationError> {
        let trimmed = raw.trim();
        if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
            return Err(SignatureVerificationError::MalformedSignatureFile {
                skill_id: String::new(),
                detail: "pubkey must not use 0x prefix".into(),
            });
        }
        let decoded = hex::decode(trimmed).map_err(|err| SignatureVerificationError::MalformedSignatureFile {
            skill_id: String::new(),
            detail: format!("invalid pubkey hex: {err}"),
        })?;
        if decoded.len() != 32 {
            return Err(SignatureVerificationError::MalformedSignatureFile {
                skill_id: String::new(),
                detail: format!("pubkey must be 32 bytes, got {}", decoded.len()),
            });
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&decoded);
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn verifying_key(&self) -> Result<VerifyingKey, SignatureVerificationError> {
        VerifyingKey::from_bytes(self.as_bytes()).map_err(|_| SignatureVerificationError::SignatureVerificationFailed {
            skill_id: String::new(),
        })
    }
}

impl fmt::Display for Ed25519PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_0x_prefix() {
        let err = Ed25519PublicKey::from_hex("0x00").expect_err("0x prefix");
        assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
    }
}
