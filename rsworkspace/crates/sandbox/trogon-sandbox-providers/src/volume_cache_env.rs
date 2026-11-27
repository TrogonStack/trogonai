//! Settings of the optional S3-backed clone cache, read from the environment.

pub(crate) mod constants;

use trogon_std::env::ReadEnv;

use constants::{BUCKET_ENV, DEFAULT_REGION, ENDPOINT_ENV, KMS_KEY_ID_ENV, REGION_ENV};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeCacheEnv {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub kms_key_id: Option<String>,
}

impl VolumeCacheEnv {
    /// Reads the cache settings, or `None` when no bucket is configured.
    pub fn from_env(env: &impl ReadEnv) -> Option<Self> {
        let bucket = env.var(BUCKET_ENV).ok()?;
        Some(Self {
            bucket,
            region: env.var(REGION_ENV).unwrap_or_else(|_| DEFAULT_REGION.to_string()),
            endpoint: env.var(ENDPOINT_ENV).ok(),
            kms_key_id: env.var(KMS_KEY_ID_ENV).ok(),
        })
    }
}
