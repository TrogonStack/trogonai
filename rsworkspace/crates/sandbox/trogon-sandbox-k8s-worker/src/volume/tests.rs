use std::collections::HashMap;
use std::env::VarError;
use std::ffi::OsString;

use trogon_std::env::ReadEnv;

use super::*;

struct FakeEnv(HashMap<&'static str, Result<String, VarError>>);

impl ReadEnv for FakeEnv {
    fn var(&self, key: &str) -> Result<String, VarError> {
        self.0.get(key).cloned().unwrap_or(Err(VarError::NotPresent))
    }
}

fn not_unicode() -> Result<String, VarError> {
    Err(VarError::NotUnicode(OsString::from("bad")))
}

#[test]
fn unset_variables_use_the_documented_defaults() -> Result<(), VolumeSyncError> {
    let config = volume_client_config_for_bucket(&FakeEnv(HashMap::new()), "bucket")?;
    assert!(config.kms_key_id.is_none());
    Ok(())
}

#[test]
fn a_non_unicode_endpoint_is_refused_not_treated_as_unset() {
    let env = FakeEnv(HashMap::from([(S3_ENDPOINT_ENV, not_unicode())]));
    let error = volume_client_config_for_bucket(&env, "bucket").err();
    assert!(matches!(
        error,
        Some(VolumeSyncError::EnvNotUnicode { name: S3_ENDPOINT_ENV })
    ));
}

#[test]
fn a_non_unicode_kms_key_is_refused_not_treated_as_unset() {
    let env = FakeEnv(HashMap::from([(S3_KMS_KEY_ID_ENV, not_unicode())]));
    let error = volume_client_config_for_bucket(&env, "bucket").err();
    assert!(matches!(
        error,
        Some(VolumeSyncError::EnvNotUnicode {
            name: S3_KMS_KEY_ID_ENV
        })
    ));
}

#[test]
fn a_non_unicode_region_is_refused_not_defaulted() {
    let env = FakeEnv(HashMap::from([(S3_REGION_ENV, not_unicode())]));
    let error = volume_client_config_for_bucket(&env, "bucket").err();
    assert!(matches!(
        error,
        Some(VolumeSyncError::EnvNotUnicode { name: S3_REGION_ENV })
    ));
}
