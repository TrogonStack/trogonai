use std::fmt;

use crate::jwt::SigningKey;

use super::key_version::KeyVersion;
use super::minting_material::MintingMaterial;
use super::verifying_key::VerifyingKey;

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

    /// The public half of this key, for a verifier that must not hold the seed.
    pub fn verifying_key(&self) -> VerifyingKey {
        VerifyingKey::from_handle(self)
    }

    pub fn minting_material(&self) -> MintingMaterial {
        MintingMaterial::new(self.key.keypair().clone(), self.version.clone())
    }
}
