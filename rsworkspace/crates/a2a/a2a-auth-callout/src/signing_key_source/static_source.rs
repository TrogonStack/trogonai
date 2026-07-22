use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;

use super::SigningKeySource;
use super::key_version::KeyVersion;
use super::signing_key_handle::SigningKeyHandle;

#[derive(Debug)]
pub struct StaticSigningKeySource {
    current: SigningKeyHandle,
    previous: Option<SigningKeyHandle>,
}

impl StaticSigningKeySource {
    pub fn new(seed: &str, version: KeyVersion) -> Result<Self, AuthCalloutError> {
        Ok(Self {
            current: SigningKeyHandle::new(version, SigningKey::from_seed(seed)?),
            previous: None,
        })
    }

    pub fn with_overlap(
        current_seed: &str,
        current_version: KeyVersion,
        previous_seed: &str,
        previous_version: KeyVersion,
    ) -> Result<Self, AuthCalloutError> {
        Ok(Self {
            current: SigningKeyHandle::new(current_version, SigningKey::from_seed(current_seed)?),
            previous: Some(SigningKeyHandle::new(
                previous_version,
                SigningKey::from_seed(previous_seed)?,
            )),
        })
    }
}

impl SigningKeySource for StaticSigningKeySource {
    fn current(&self) -> SigningKeyHandle {
        self.current.clone()
    }

    fn accepted(&self) -> Vec<SigningKeyHandle> {
        let mut keys = vec![self.current.clone()];
        if let Some(prev) = &self.previous {
            keys.push(prev.clone());
        }
        keys
    }
}
