use crate::jwt::SigningKey;

use super::key_version::KeyVersion;
use super::signing_key_handle::SigningKeyHandle;
use super::SigningKeySource;

#[derive(Debug)]
pub struct StaticSigningKeySource {
    current: SigningKeyHandle,
    previous: Option<SigningKeyHandle>,
}

impl StaticSigningKeySource {
    pub fn new(secret: &[u8], version: KeyVersion) -> Self {
        Self {
            current: SigningKeyHandle::new(version, SigningKey::from_secret(secret)),
            previous: None,
        }
    }

    pub fn with_overlap(
        current_secret: &[u8],
        current_version: KeyVersion,
        previous_secret: &[u8],
        previous_version: KeyVersion,
    ) -> Self {
        Self {
            current: SigningKeyHandle::new(
                current_version,
                SigningKey::from_secret(current_secret),
            ),
            previous: Some(SigningKeyHandle::new(
                previous_version,
                SigningKey::from_secret(previous_secret),
            )),
        }
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
