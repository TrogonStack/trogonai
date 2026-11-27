//! Optional S3-backed clone cache for sandbox workspace setup.

#[cfg(feature = "volume-cache")]
mod cache_client;

use std::future::Future;
use std::path::Path;

use crate::{CloneCredentials, SandboxSpec, VolumeCacheError};

#[cfg(feature = "volume-cache")]
use trogon_sandbox_volumes::{CloneCacheOutcome, VolumeClient, populate_clone_cache, try_pull_cached_clone};

/// Runs `git_clone`, first consulting the object-store clone cache when the
/// `volume-cache` feature is enabled. A spec without clone credentials has
/// nothing to clone and succeeds without running `git_clone`.
#[cfg(feature = "volume-cache")]
pub async fn try_clone_with_volume_cache<E>(
    spec: &SandboxSpec,
    workspace_path: &Path,
    git_clone: impl Future<Output = Result<(), E>>,
) -> Result<(), E>
where
    E: From<VolumeCacheError>,
{
    clone_through_cache(cache_client::client_from_env, spec, workspace_path, git_clone).await
}

/// Runs `git_clone`; this build has no clone cache.
#[cfg(not(feature = "volume-cache"))]
pub async fn try_clone_with_volume_cache<E>(
    _spec: &SandboxSpec,
    _workspace_path: &Path,
    git_clone: impl Future<Output = Result<(), E>>,
) -> Result<(), E>
where
    E: From<VolumeCacheError>,
{
    git_clone.await
}

#[cfg(feature = "volume-cache")]
async fn clone_through_cache<E>(
    setup: impl FnOnce() -> Result<VolumeClient, VolumeCacheError>,
    spec: &SandboxSpec,
    workspace_path: &Path,
    git_clone: impl Future<Output = Result<(), E>>,
) -> Result<(), E>
where
    E: From<VolumeCacheError>,
{
    let Some(credentials) = clone_credentials(spec) else {
        return Ok(());
    };
    let client = setup()?;
    let url = credentials.clone_url.as_str();
    let sha = spec.resolved_sha.as_str();

    match try_pull_cached_clone(&client, url, sha, workspace_path).await {
        Ok(CloneCacheOutcome::Hit) => return Ok(()),
        Ok(CloneCacheOutcome::Miss) => {}
        Err(error) => {
            tracing::warn!(%error, "clone cache pull failed; falling back to git");
        }
    }

    git_clone.await?;
    if let Err(error) = populate_clone_cache(&client, url, sha, workspace_path).await {
        tracing::warn!(%error, "clone cache populate failed");
    }
    Ok(())
}

/// The clone credentials of `spec`, if it clones a repository.
pub fn clone_credentials(spec: &SandboxSpec) -> Option<&CloneCredentials> {
    spec.clone.as_ref()
}

#[cfg(all(test, feature = "volume-cache"))]
mod cache_tests;

#[cfg(test)]
mod tests;
