use std::fmt;

use super::error::SignedExportError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperatorKeyId(String);

impl OperatorKeyId {
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, SignedExportError> {
        let raw = raw.as_ref();
        if is_valid_operator_key_id(raw) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(SignedExportError::Malformed(format!(
                "operator key id `{raw}` must match ^[A-Za-z0-9._-]{{1,64}}$"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid_operator_key_id(raw: &str) -> bool {
    let len = raw.len();
    if len == 0 || len > 64 {
        return false;
    }
    raw.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_' || byte == b'-'
    })
}

impl fmt::Display for OperatorKeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ed25519PublicKey([u8; 32]);

impl Ed25519PublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn parse_hex(raw: impl AsRef<str>) -> Result<Self, SignedExportError> {
        let raw = raw.as_ref();
        let decoded = hex::decode(raw).map_err(|error| {
            SignedExportError::Malformed(format!("operator public key hex decode failed: {error}"))
        })?;
        let bytes: [u8; 32] = decoded
            .try_into()
            .map_err(|_| SignedExportError::Malformed("operator public key must be 32 bytes".into()))?;
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
