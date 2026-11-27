//! How an export target picks up its encryption key from the environment.

use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_daytona_worker::{S3ExportError, S3ExportTarget};
use trogon_sandbox_volumes::{VolumePrefix, VolumeRegion};
use trogon_std::env::InMemoryEnv;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn target() -> Result<S3ExportTarget, Box<dyn std::error::Error>> {
    Ok(S3ExportTarget::new(
        S3BucketName::new("exports")?,
        VolumePrefix::new("lease-1")?,
        VolumeRegion::new("us-east-1")?,
        None,
    ))
}

#[test]
fn an_unset_key_variable_means_no_key() -> TestResult {
    let target = target()?.with_kms_key_from_env(&InMemoryEnv::new())?;
    assert!(target.kms_key_id.is_none());
    Ok(())
}

#[test]
fn a_set_key_variable_names_the_key() -> TestResult {
    let env = InMemoryEnv::new();
    env.set(S3ExportTarget::KMS_KEY_ENV, "key-1");
    let target = target()?.with_kms_key_from_env(&env)?;
    assert_eq!(target.kms_key_id.as_ref().map(|key| key.as_str()), Some("key-1"));
    Ok(())
}

#[test]
fn a_blank_key_variable_is_refused() -> TestResult {
    let env = InMemoryEnv::new();
    env.set(S3ExportTarget::KMS_KEY_ENV, "  ");
    let refused = target()?.with_kms_key_from_env(&env);
    assert!(matches!(refused, Err(S3ExportError::KmsKey(_))));
    Ok(())
}
