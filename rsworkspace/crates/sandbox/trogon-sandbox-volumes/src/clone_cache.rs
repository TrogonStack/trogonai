//! Transparent object-store cache for clone workspaces, consulted before running `git clone`.

use std::path::Path;

use trogon_sandbox_contract::{VolumeMode, WorkspaceSource};

use crate::{
    CloneCacheConfig, CloneCacheOutcome, VolumeBackend, VolumeClient, VolumeClientConfig, VolumeError, VolumeMetrics,
    VolumePrefix, VolumePrefixError, build_changed_manifest,
};

/// The prefix a repository URL at a commit is cached under.
pub fn clone_cache_prefix(url: &str, sha: &str) -> Result<VolumePrefix, VolumePrefixError> {
    let normalized_url = url.trim_end_matches('/').replace("://", "/");
    VolumePrefix::new(format!("clone-cache/{normalized_url}/{sha}"))
}

pub async fn try_pull_cached_clone(
    client: &VolumeClient,
    url: &str,
    sha: &str,
    dest: &Path,
) -> Result<CloneCacheOutcome, VolumeError> {
    let prefix = clone_cache_prefix(url, sha)?;
    if !client.head_exists(&prefix).await? {
        return Ok(CloneCacheOutcome::Miss);
    }
    client.pull_to_dir(&prefix, dest).await?;
    client.record_cache_hit();
    Ok(CloneCacheOutcome::Hit)
}

pub fn volume_client_for_clone_cache(config: &CloneCacheConfig) -> Result<VolumeClient, VolumeError> {
    VolumeClient::with_options(
        VolumeClientConfig {
            backend: VolumeBackend::S3 {
                bucket: config.bucket.clone(),
                region: config.region.clone(),
                endpoint: config.endpoint.clone(),
            },
            kms_key_id: config.kms_key_id.clone(),
        },
        VolumeMode::ReadOnly,
        VolumeMetrics::new()?,
        None,
    )
}

/// The repository URL and commit of a clone source, or `None` for any other source.
pub fn clone_cache_key(source: &WorkspaceSource) -> Option<(&str, &str)> {
    match source {
        WorkspaceSource::Clone { url, sha, .. } => Some((url.as_str(), sha.as_str())),
        _ => None,
    }
}

pub async fn populate_clone_cache(client: &VolumeClient, url: &str, sha: &str, src: &Path) -> Result<(), VolumeError> {
    let prefix = clone_cache_prefix(url, sha)?;
    let manifest = build_changed_manifest(src, None)?;
    client.push_changed(&prefix, src, manifest).await?;
    Ok(())
}
