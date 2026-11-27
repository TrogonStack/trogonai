use std::path::Path;

use tempfile::TempDir;
use trogon_sandbox_contract::{CredentialScope, S3BucketName, TenantId, VolumeMode, WorkspaceSource};
use trogon_sandbox_volumes::VolumePrefix;

use crate::{CloneUrl, CommitSha, VolumeStores, VolumeSyncPlan, WorkspaceError, git_checkout, volume_transfer};

/// A workspace staged in worker-local scratch, ready to be uploaded into a sandbox.
///
/// A Modal sandbox is remote, so nothing on this host is visible to it. A read-write volume remembers how to
/// write its changes back.
#[derive(Debug)]
pub(crate) struct MaterializedWorkspace {
    scratch: TempDir,
    sync: Option<VolumeSyncPlan>,
}

impl MaterializedWorkspace {
    pub(crate) async fn materialize(
        source: &WorkspaceSource,
        tenant: &TenantId,
        stores: &dyn VolumeStores,
    ) -> Result<Self, WorkspaceError> {
        let scratch = TempDir::new()?;
        let sync = match source {
            WorkspaceSource::Empty(_) => None,
            WorkspaceSource::Clone { url, sha, auth, .. } => {
                let url = CloneUrl::new(url, auth)?;
                git_checkout::clone_checkout(&url, &CommitSha::new(sha.as_str())?, scratch.path()).await?;
                None
            }
            WorkspaceSource::S3Volume {
                bucket, prefix, mode, ..
            } => {
                let bucket = S3BucketName::new(bucket.as_str())?;
                let prefix = VolumePrefix::new(prefix)?;
                let (scope, writable) = scope_for(bucket.as_str(), &prefix, *mode);
                let store = stores.open(tenant, &scope).await?;
                volume_transfer::pull(store.as_ref(), &prefix, scratch.path()).await?;
                writable.then_some(VolumeSyncPlan { scope, prefix })
            }
            WorkspaceSource::HostMount { .. } => return Err(WorkspaceError::HostMountUnsupported),
        };
        Ok(Self { scratch, sync })
    }

    pub(crate) fn local_path(&self) -> &Path {
        self.scratch.path()
    }

    /// Writes a read-write volume's changes back. Other workspaces have nothing to write.
    pub(crate) async fn push_changes(
        &self,
        tenant: &TenantId,
        stores: &dyn VolumeStores,
    ) -> Result<(), WorkspaceError> {
        let Some(plan) = &self.sync else {
            return Ok(());
        };
        let store = stores.open(tenant, &plan.scope).await?;
        volume_transfer::push(store.as_ref(), &plan.prefix, self.local_path()).await
    }
}

/// The credential scope a volume mode needs, and whether its changes are written back.
fn scope_for(bucket: &str, prefix: &VolumePrefix, mode: VolumeMode) -> (CredentialScope, bool) {
    let (bucket, prefix) = (bucket.to_owned(), prefix.as_str().to_owned());
    match mode {
        VolumeMode::ReadOnly => (CredentialScope::S3ReadOnly { bucket, prefix }, false),
        VolumeMode::ReadWrite => (CredentialScope::S3ReadWrite { bucket, prefix }, true),
        VolumeMode::CopyOnWrite => (CredentialScope::S3ReadWrite { bucket, prefix }, false),
    }
}
