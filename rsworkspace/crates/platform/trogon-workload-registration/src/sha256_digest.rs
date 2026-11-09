use sha2::{Digest, Sha256};

use crate::RegistrationError;
use crate::constants::HEX_ALPHABET;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RegistrationError> {
        Ok(Self(bytes.try_into().map_err(|_| RegistrationError::Bounds)?))
    }

    pub fn hash(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        self.0
            .iter()
            .flat_map(|byte| [HEX_ALPHABET[(byte >> 4) as usize], HEX_ALPHABET[(byte & 15) as usize]])
            .map(char::from)
            .collect()
    }
}
