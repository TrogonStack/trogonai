use std::sync::Arc;

use async_trait::async_trait;

use super::{KeyError, KeySource};
use crate::jwks::Jwks;
use crate::signer::MeshSigner;
use crate::signer::vault::VaultSigner;

pub struct VaultKeySource {
    signer: Arc<VaultSigner>,
}

impl VaultKeySource {
    pub async fn new(
        vault_addr: impl Into<String>,
        mount: impl Into<String>,
        key_name: impl Into<String>,
        token: impl Into<String>,
        issuer: Option<String>,
    ) -> Result<Self, KeyError> {
        let signer = VaultSigner::new(vault_addr, mount, key_name, token, issuer)?;
        let signer = signer.bootstrap().await?;
        Ok(Self {
            signer: Arc::new(signer),
        })
    }

    pub fn from_signer(signer: Arc<VaultSigner>) -> Self {
        Self { signer }
    }

    pub fn signer(&self) -> Arc<VaultSigner> {
        Arc::clone(&self.signer)
    }
}

#[async_trait]
impl KeySource for VaultKeySource {
    async fn current(&self) -> Result<Jwks, KeyError> {
        self.signer.active_jwks().await
    }
}
