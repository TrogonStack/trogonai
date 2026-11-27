use tempfile::TempDir;
use trogon_sandbox_contract::{TenantId, WorkspaceSource};
use trogon_sandbox_volumes::{StoreResourceName, VolumeMode, VolumePrefix};

use crate::{AcaError, MaterializedWorkspace, VolumeSupport, VolumeSyncPlan, clone_git_workspace};

/// Lays the workspace out in a fresh scratch directory.
pub async fn materialize_workspace(
    source: &WorkspaceSource,
    tenant: &TenantId,
    volumes: &VolumeSupport,
) -> Result<MaterializedWorkspace, AcaError> {
    let scratch = TempDir::new()?;
    let volume_sync = match source {
        WorkspaceSource::Empty(_) => None,
        WorkspaceSource::Clone { url, sha, auth, .. } => {
            clone_git_workspace(url, sha, auth, scratch.path()).await?;
            None
        }
        WorkspaceSource::S3Volume {
            bucket, prefix, mode, ..
        } => {
            let account = volumes.storage_account.clone().ok_or_else(|| {
                AcaError::InvalidSpec("S3Volume on ACA requires SANDBOX_ACA_VOLUME_STORAGE_ACCOUNT".to_owned())
            })?;
            let container = StoreResourceName::new(bucket)?;
            let prefix = VolumePrefix::new(prefix)?;
            volumes
                .client(tenant, &container, &account, &prefix, *mode)
                .await?
                .pull_to_dir(&prefix, scratch.path())
                .await?;
            (*mode == VolumeMode::ReadWrite).then_some(VolumeSyncPlan {
                container,
                prefix,
                mode: *mode,
                account,
            })
        }
        WorkspaceSource::HostMount { .. } => {
            return Err(AcaError::InvalidSpec(
                "host_mount workspaces are not supported on the ACA provider".to_owned(),
            ));
        }
    };
    Ok(MaterializedWorkspace { scratch, volume_sync })
}
