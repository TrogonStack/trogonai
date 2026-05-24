use std::fmt;

use ed25519_dalek::Signature;

use super::error::SignatureVerificationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ed25519Signature([u8; 64]);

impl Ed25519Signature {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(raw: &str) -> Result<Self, SignatureVerificationError> {
        let decoded = hex::decode(raw).map_err(|err| SignatureVerificationError::MalformedSignatureFile {
            skill_id: String::new(),
            detail: format!("invalid signature hex: {err}"),
        })?;
        if decoded.len() != 64 {
            return Err(SignatureVerificationError::MalformedSignatureFile {
                skill_id: String::new(),
                detail: format!("signature must be 64 bytes, got {}", decoded.len()),
            });
        }
        let mut bytes = [0u8; 64];
        bytes.copy_from_slice(&decoded);
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    pub fn dalek_signature(&self) -> Result<Signature, SignatureVerificationError> {
        Signature::from_slice(self.as_bytes()).map_err(|_| SignatureVerificationError::MalformedSignatureFile {
            skill_id: String::new(),
            detail: "signature bytes are not canonical ed25519".into(),
        })
    }
}

impl fmt::Display for Ed25519Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}
