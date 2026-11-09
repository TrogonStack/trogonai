use ed25519_dalek::VerifyingKey;

use crate::RegistrationError;
use crate::constants::{DID_KEY_PREFIX, ED25519_MULTICODEC, MAX_DID_KEY_BYTES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalAgentId(String);

impl CanonicalAgentId {
    pub fn new(value: &str) -> Result<Self, RegistrationError> {
        if value.len() > MAX_DID_KEY_BYTES {
            return Err(RegistrationError::ApplicationIdentity);
        }
        let encoded = value
            .strip_prefix(DID_KEY_PREFIX)
            .ok_or(RegistrationError::ApplicationIdentity)?;
        let bytes = bs58::decode(encoded).into_vec().map_err(RegistrationError::Base58)?;
        if bytes.len() != 34 || bytes[..2] != ED25519_MULTICODEC || bs58::encode(&bytes).into_string() != encoded {
            return Err(RegistrationError::ApplicationIdentity);
        }
        let key = VerifyingKey::from_bytes(
            bytes[2..]
                .try_into()
                .map_err(|_| RegistrationError::ApplicationIdentity)?,
        )
        .map_err(RegistrationError::ApplicationKey)?;
        if key.is_weak() {
            return Err(RegistrationError::ApplicationIdentity);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
