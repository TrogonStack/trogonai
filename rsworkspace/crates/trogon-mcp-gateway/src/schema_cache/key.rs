use std::fmt;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerId(String);

impl ServerId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaHash([u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaHashError {
    InvalidHex(String),
    InvalidLength { expected: usize, actual: usize },
}

impl fmt::Display for SchemaHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHex(message) => write!(f, "invalid schema hash hex: {message}"),
            Self::InvalidLength { expected, actual } => {
                write!(f, "schema hash must be {expected} bytes, got {actual}")
            }
        }
    }
}

impl std::error::Error for SchemaHashError {}

impl SchemaHash {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(raw: &str) -> Result<Self, SchemaHashError> {
        let decoded = hex::decode(raw).map_err(|err| SchemaHashError::InvalidHex(err.to_string()))?;
        if decoded.len() != 32 {
            return Err(SchemaHashError::InvalidLength {
                expected: 32,
                actual: decoded.len(),
            });
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&decoded);
        Ok(Self(bytes))
    }
}

impl Hash for SchemaHash {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Display for SchemaHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_hex())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SchemaCacheKey {
    pub server_id: ServerId,
    pub schema_hash: SchemaHash,
}

impl fmt::Display for SchemaCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.server_id, self.schema_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_hash_hex_roundtrip() {
        let hash = SchemaHash::from_bytes([0xab; 32]);
        let parsed = SchemaHash::from_hex(&hash.as_hex()).expect("valid hex");
        assert_eq!(parsed, hash);
    }

    #[test]
    fn schema_hash_rejects_invalid_hex() {
        let err = SchemaHash::from_hex("not-hex").expect_err("invalid hex");
        assert!(matches!(err, SchemaHashError::InvalidHex(_)));

        let err = SchemaHash::from_hex("abcd").expect_err("wrong length");
        assert!(matches!(err, SchemaHashError::InvalidLength { .. }));
    }
}
