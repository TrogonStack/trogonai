use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_volumes::{KmsKeyId, VolumeBackend, VolumeClientConfig, VolumeEndpoint, VolumePrefix, VolumeRegion};
use trogon_std::env::ReadEnv;

use crate::S3ExportError;

/// Where an exported workspace lands: a bucket, a prefix inside it, and the optional server-side encryption key.
#[derive(Debug, Clone)]
pub struct S3ExportTarget {
    pub bucket: S3BucketName,
    pub prefix: VolumePrefix,
    pub region: VolumeRegion,
    pub endpoint: Option<VolumeEndpoint>,
    pub kms_key_id: Option<KmsKeyId>,
}

impl S3ExportTarget {
    /// Environment variable that names the KMS key the export encrypts with.
    pub const KMS_KEY_ENV: &'static str = "SANDBOX_S3_KMS_KEY_ID";

    pub fn new(
        bucket: S3BucketName,
        prefix: VolumePrefix,
        region: VolumeRegion,
        endpoint: Option<VolumeEndpoint>,
    ) -> Self {
        Self {
            bucket,
            prefix,
            region,
            endpoint,
            kms_key_id: None,
        }
    }

    #[must_use]
    pub fn with_kms_key_id(mut self, kms_key_id: Option<KmsKeyId>) -> Self {
        self.kms_key_id = kms_key_id;
        self
    }

    /// Reads the KMS key from [`Self::KMS_KEY_ENV`]; an unset variable means no key.
    pub fn with_kms_key_from_env(self, env: &impl ReadEnv) -> Result<Self, S3ExportError> {
        let key = env.var(Self::KMS_KEY_ENV).ok().map(KmsKeyId::new).transpose()?;
        Ok(self.with_kms_key_id(key))
    }

    pub(crate) fn client_config(&self) -> VolumeClientConfig {
        VolumeClientConfig {
            backend: VolumeBackend::S3 {
                bucket: self.bucket.clone(),
                region: self.region.clone(),
                endpoint: self.endpoint.clone(),
            },
            kms_key_id: self.kms_key_id.clone(),
        }
    }
}
