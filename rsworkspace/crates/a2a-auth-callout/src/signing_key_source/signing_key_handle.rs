use std::fmt;

use crate::jwt::SigningKey;

use super::key_version::KeyVersion;

#[derive(Clone)]
pub struct SigningKeyHandle {
    version: KeyVersion,
    key: SigningKey,
}

impl fmt::Debug for SigningKeyHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SigningKeyHandle")
            .field("version", &self.version)
            .field("key", &self.key)
            .finish()
    }
}

impl SigningKeyHandle {
    pub(crate) fn new(version: KeyVersion, key: SigningKey) -> Self {
        Self { version, key }
    }

    pub fn version(&self) -> &KeyVersion {
        &self.version
    }

    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.key
    }
}
