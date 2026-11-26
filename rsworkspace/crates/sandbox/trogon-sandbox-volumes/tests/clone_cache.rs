#![cfg(feature = "volume-cache")]

use tempfile::tempdir;
use trogon_sandbox_contract::{CloneAuth, EmptyObject, S3BucketName, WorkspaceSource};
use trogon_sandbox_volumes::{
    CloneCacheConfig, CloneCacheOutcome, VolumeBackend, VolumeClient, VolumeClientConfig, VolumeMode, VolumeRegion,
    clone_cache_key, clone_cache_prefix, populate_clone_cache, try_pull_cached_clone, volume_client_for_clone_cache,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn prefix_embeds_the_url_and_commit() -> TestResult {
    assert_eq!(
        clone_cache_prefix("https://example.com/org/repo/", "abc123")?.as_str(),
        "clone-cache/https/example.com/org/repo/abc123"
    );
    assert!(clone_cache_prefix("https://example.com/../x", "abc").is_err());
    Ok(())
}

#[test]
fn only_clone_sources_have_a_cache_key() {
    let clone = WorkspaceSource::Clone {
        url: "https://example.com/r".to_owned(),
        sha: "abc".to_owned(),
        base_sha: None,
        auth: CloneAuth::None(EmptyObject::default()),
    };
    assert_eq!(clone_cache_key(&clone), Some(("https://example.com/r", "abc")));
    assert_eq!(clone_cache_key(&WorkspaceSource::empty()), None);
}

#[test]
fn cache_client_targets_the_configured_bucket() -> TestResult {
    let client = volume_client_for_clone_cache(&CloneCacheConfig {
        bucket: S3BucketName::new("cache-bucket")?,
        region: VolumeRegion::new("us-east-1")?,
        endpoint: None,
        kms_key_id: None,
    })?;
    assert_eq!(client.bucket(), "cache-bucket");
    assert_eq!(client.mode(), VolumeMode::ReadOnly);
    Ok(())
}

#[tokio::test]
async fn populated_entries_are_served_as_hits() -> TestResult {
    let store = tempdir()?;
    let client = VolumeClient::with_mode(
        VolumeClientConfig {
            backend: VolumeBackend::LocalFs {
                root: store.path().to_path_buf(),
            },
            kms_key_id: None,
        },
        VolumeMode::ReadWrite,
    )?;
    let url = "https://example.com/org/repo";

    let miss = tempdir()?;
    assert_eq!(
        try_pull_cached_clone(&client, url, "abc", miss.path()).await?,
        CloneCacheOutcome::Miss
    );

    let src = tempdir()?;
    std::fs::write(src.path().join("a.txt"), b"a")?;
    populate_clone_cache(&client, url, "abc", src.path()).await?;

    let hit = tempdir()?;
    assert_eq!(
        try_pull_cached_clone(&client, url, "abc", hit.path()).await?,
        CloneCacheOutcome::Hit
    );
    assert!(hit.path().join("a.txt").exists());
    assert!(
        client
            .metrics()
            .gather_text()?
            .contains("trogon_sandbox_volumes_cache_hit_total")
    );
    Ok(())
}
