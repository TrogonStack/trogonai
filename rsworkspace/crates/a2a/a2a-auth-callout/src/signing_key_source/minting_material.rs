use nkeys::KeyPair;

use super::key_version::KeyVersion;

/// Account NKey material used to mint and verify inner NATS User JWTs.
#[derive(Clone)]
pub struct MintingMaterial {
    issuer: KeyPair,
    version: KeyVersion,
}

impl MintingMaterial {
    pub(crate) fn new(issuer: KeyPair, version: KeyVersion) -> Self {
        Self { issuer, version }
    }

    pub fn version(&self) -> &KeyVersion {
        &self.version
    }

    pub fn issuer_public(&self) -> String {
        self.issuer.public_key()
    }

    pub(crate) fn issuer_keypair(&self) -> &KeyPair {
        &self.issuer
    }
}

impl std::fmt::Debug for MintingMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintingMaterial")
            .field("version", &self.version)
            .field("issuer_public", &self.issuer.public_key())
            .finish()
    }
}
