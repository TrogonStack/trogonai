use nkeys::KeyPair;

use super::key_version::KeyVersion;
use crate::admission_keys::{AccountSigningKey, TenantAccount};

/// Account NKey material used to mint and verify inner NATS User JWTs.
#[derive(Clone)]
pub struct MintingMaterial {
    issuer: KeyPair,
    version: KeyVersion,
    issuer_account: Option<TenantAccount>,
}

impl MintingMaterial {
    pub(crate) fn new(issuer: KeyPair, version: KeyVersion) -> Self {
        Self {
            issuer,
            version,
            issuer_account: None,
        }
    }

    pub(crate) fn on_behalf_of(signing_key: &AccountSigningKey, tenant: TenantAccount) -> Self {
        Self {
            issuer: signing_key.keypair().clone(),
            version: KeyVersion::current(),
            issuer_account: Some(tenant),
        }
    }

    pub fn version(&self) -> &KeyVersion {
        &self.version
    }

    pub fn issuer_public(&self) -> String {
        self.issuer.public_key()
    }

    pub fn issuer_account(&self) -> Option<&TenantAccount> {
        self.issuer_account.as_ref()
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
            .field("issuer_account", &self.issuer_account)
            .finish()
    }
}
