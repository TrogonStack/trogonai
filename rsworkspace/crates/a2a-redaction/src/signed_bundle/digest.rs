use std::fmt;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn hash(payload: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(payload);
        Self(hasher.finalize().into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_hex(raw: &str) -> Result<Self, hex::FromHexError> {
        let decoded = hex::decode(raw)?;
        if decoded.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&decoded);
        Ok(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic() {
        let first = Sha256Digest::hash(b"payload");
        let second = Sha256Digest::hash(b"payload");
        assert_eq!(first, second);
    }

    #[test]
    fn hex_roundtrip() {
        let digest = Sha256Digest::hash(b"x");
        let parsed = Sha256Digest::from_hex(&digest.to_hex()).expect("parse hex");
        assert_eq!(parsed, digest);
    }
}
