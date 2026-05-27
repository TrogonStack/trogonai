use std::sync::Arc;

use async_trait::async_trait;

use super::{KeyError, KeySource};
use crate::jwks::Jwks;
use crate::signer::kms::KmsSigner;
use crate::signer::MeshSigner;

pub struct KmsKeySource {
    signer: Arc<KmsSigner>,
}

impl KmsKeySource {
    pub async fn new(
        key_arn: impl Into<String>,
        region: Option<String>,
        previous_key_version: Option<String>,
        issuer: Option<String>,
    ) -> Result<Self, KeyError> {
        let signer = KmsSigner::new(key_arn, region, previous_key_version, issuer).await?;
        Ok(Self {
            signer: Arc::new(signer),
        })
    }

    pub fn from_signer(signer: Arc<KmsSigner>) -> Self {
        Self { signer }
    }

    pub fn signer(&self) -> Arc<KmsSigner> {
        Arc::clone(&self.signer)
    }
}

#[async_trait]
impl KeySource for KmsKeySource {
    async fn current(&self) -> Result<Jwks, KeyError> {
        self.signer.active_jwks().await
    }
}
