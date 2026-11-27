type TestResult = Result<(), Box<dyn std::error::Error>>;

use trogon_sandbox_contract::VolumeMode;
use trogon_sandbox_volumes::{VolumeBackend, VolumeClient, VolumeClientConfig};
use trogon_std::env::InMemoryEnv;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::tests::spec;
use super::*;
use crate::SubprocessError;
use crate::volume_cache_env::constants::{BUCKET_ENV, ENDPOINT_ENV, KMS_KEY_ID_ENV, REGION_ENV};

fn local_client(root: &Path) -> Result<VolumeClient, VolumeCacheError> {
    VolumeClient::with_mode(
        VolumeClientConfig {
            backend: VolumeBackend::LocalFs {
                root: root.to_path_buf(),
            },
            kms_key_id: None,
        },
        VolumeMode::ReadWrite,
    )
    .map_err(|error| VolumeCacheError::Client(Box::new(error)))
}

async fn counted_git(counter: &AtomicUsize, fail: bool) -> Result<(), SubprocessError> {
    counter.fetch_add(1, Ordering::SeqCst);
    if fail {
        Err(SubprocessError::HostMountUnsupported)
    } else {
        Ok(())
    }
}

#[tokio::test]
async fn a_spec_without_a_clone_never_builds_a_client_or_runs_git() -> TestResult {
    let runs = AtomicUsize::new(0);
    let result = clone_through_cache(
        || Err(VolumeCacheError::BucketUnset),
        &spec(false)?,
        Path::new("/unused"),
        counted_git(&runs, false),
    )
    .await;
    assert!(result.is_ok());
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_client_that_cannot_be_built_fails_the_clone() -> TestResult {
    let runs = AtomicUsize::new(0);
    let result = clone_through_cache(
        || Err(VolumeCacheError::BucketUnset),
        &spec(true)?,
        Path::new("/unused"),
        counted_git(&runs, false),
    )
    .await;
    assert!(matches!(
        result,
        Err(SubprocessError::VolumeCache(VolumeCacheError::BucketUnset))
    ));
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_miss_runs_git_populates_the_cache_and_the_next_clone_hits() -> TestResult {
    let cache = tempfile::tempdir()?;
    let first = tempfile::tempdir()?;
    std::fs::write(first.path().join("README.md"), "hello")?;
    let spec = spec(true)?;
    let runs = AtomicUsize::new(0);

    clone_through_cache(
        || local_client(cache.path()),
        &spec,
        first.path(),
        counted_git(&runs, false),
    )
    .await?;
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    let second = tempfile::tempdir()?;
    clone_through_cache(
        || local_client(cache.path()),
        &spec,
        second.path(),
        counted_git(&runs, false),
    )
    .await?;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read_to_string(second.path().join("README.md"))?, "hello");
    Ok(())
}

#[tokio::test]
async fn a_failing_pull_and_populate_fall_back_to_git() -> TestResult {
    let cache = tempfile::tempdir()?;
    let seeded = tempfile::tempdir()?;
    std::fs::write(seeded.path().join("README.md"), "hello")?;
    let spec = spec(true)?;
    let runs = AtomicUsize::new(0);
    clone_through_cache(
        || local_client(cache.path()),
        &spec,
        seeded.path(),
        counted_git(&runs, false),
    )
    .await?;

    // A destination below a regular file can neither be pulled into nor read back.
    let blocker = tempfile::NamedTempFile::new()?;
    let impossible = blocker.path().join("workspace");
    clone_through_cache(
        || local_client(cache.path()),
        &spec,
        &impossible,
        counted_git(&runs, false),
    )
    .await?;
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn a_failing_populate_after_a_miss_is_only_logged() -> TestResult {
    let cache = tempfile::tempdir()?;
    let missing = cache.path().join("never-created");
    let runs = AtomicUsize::new(0);
    clone_through_cache(
        || local_client(cache.path()),
        &spec(true)?,
        &missing,
        counted_git(&runs, false),
    )
    .await?;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_failing_git_clone_is_the_result() -> TestResult {
    let cache = tempfile::tempdir()?;
    let runs = AtomicUsize::new(0);
    let result = clone_through_cache(
        || local_client(cache.path()),
        &spec(true)?,
        cache.path(),
        counted_git(&runs, true),
    )
    .await;
    assert!(matches!(result, Err(SubprocessError::HostMountUnsupported)));
    Ok(())
}

#[tokio::test]
async fn the_public_entry_point_without_a_clone_succeeds() -> TestResult {
    let runs = AtomicUsize::new(0);
    try_clone_with_volume_cache(&spec(false)?, Path::new("/unused"), counted_git(&runs, false)).await?;
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn the_client_is_built_from_the_environment_settings() -> TestResult {
    let env = InMemoryEnv::new();
    env.set(BUCKET_ENV, "cache");
    env.set(REGION_ENV, "eu-west-1");
    env.set(ENDPOINT_ENV, "http://127.0.0.1:9000");
    env.set(KMS_KEY_ID_ENV, "key-1");
    let client = cache_client::client_from(&env)?;
    assert_eq!(client.bucket(), "cache");
    Ok(())
}

#[test]
fn a_missing_bucket_is_reported() {
    assert!(matches!(
        cache_client::client_from(&InMemoryEnv::new()),
        Err(VolumeCacheError::BucketUnset)
    ));
}

#[test]
fn the_process_environment_has_no_bucket_in_tests() {
    assert!(cache_client::client_from_env().is_err());
}

#[test]
fn each_invalid_setting_names_its_variable() {
    for variable in [BUCKET_ENV, REGION_ENV, ENDPOINT_ENV, KMS_KEY_ID_ENV] {
        let env = InMemoryEnv::new();
        env.set(BUCKET_ENV, "cache");
        env.set(variable, "");
        let result = cache_client::client_from(&env);
        assert!(
            matches!(&result, Err(VolumeCacheError::InvalidSetting { variable: named, .. }) if *named == variable),
            "{variable} was not rejected: {:?}",
            result.map(|_| ())
        );
    }
}
