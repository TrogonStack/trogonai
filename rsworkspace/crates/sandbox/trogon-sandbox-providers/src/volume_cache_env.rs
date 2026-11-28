//! Settings of the optional S3-backed clone cache, read from the environment.

pub(crate) mod constants;

use std::env::VarError;

use trogon_std::env::ReadEnv;

use crate::VolumeCacheError;

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
    ///
    /// A variable that is set but is not valid Unicode is an error rather than an absent setting, so an unreadable
    /// KMS key or endpoint can never silently turn into an unencrypted or default-endpoint cache.
    pub fn from_env(env: &impl ReadEnv) -> Result<Option<Self>, VolumeCacheError> {
        let Some(bucket) = optional(env, BUCKET_ENV)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            bucket,
            region: optional(env, REGION_ENV)?.unwrap_or_else(|| DEFAULT_REGION.to_string()),
            endpoint: optional(env, ENDPOINT_ENV)?,
            kms_key_id: optional(env, KMS_KEY_ID_ENV)?,
        }))
    }
}

fn optional(env: &impl ReadEnv, variable: &'static str) -> Result<Option<String>, VolumeCacheError> {
    match env.var(variable) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(error @ VarError::NotUnicode(_)) => Err(VolumeCacheError::InvalidSetting {
            variable,
            source: Box::new(error),
        }),
    }
}
