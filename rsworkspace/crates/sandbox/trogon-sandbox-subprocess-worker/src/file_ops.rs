//! Structured file operations over the host side of each lease's `/workspace`.
//!
//! The workspace directory is bind-mounted into the sandbox, so the worker serves the
//! `file_op` verb straight from the host directory through [`trogon_sandbox_file_ops::FsRoot`],
//! confined to that directory. Only the workspace is reachable: other sandbox paths (system
//! directories, extra mounts) are not visible to the worker and answer `InvalidPath`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use trogon_sandbox_contract::{
    FileContentHashing, FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, FileRecursion, LeaseId,
};
use trogon_sandbox_file_ops::{FileOpError, FsRoot, Manifest, ManifestSource, SandboxPrefix, Tracking};

use crate::constants::SANDBOX_WORKSPACE_PATH;

#[derive(Debug, Clone)]
struct Workspace {
    root: PathBuf,
    readonly: bool,
}

/// Host workspace directories of the leases this worker is running.
#[derive(Clone, Default)]
pub struct WorkspaceRegistry {
    leases: Arc<Mutex<HashMap<String, Workspace>>>,
    tracking: Arc<Tracking>,
}

impl std::fmt::Debug for WorkspaceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceRegistry").finish_non_exhaustive()
    }
}

/// A workspace seen through its host directory, for checkpoints and watches.
struct HostSource {
    root: PathBuf,
}

impl HostSource {
    fn fs(&self) -> Result<FsRoot, FileOpError> {
        workspace_fs(&self.root)
    }
}

fn workspace_fs(root: &std::path::Path) -> Result<FsRoot, FileOpError> {
    let unavailable =
        |e: &dyn std::fmt::Display| FileOpError::failed(FileOpErrorKind::Io, format!("workspace is unavailable: {e}"));
    let prefix = SandboxPrefix::new(SANDBOX_WORKSPACE_PATH).map_err(|e| unavailable(&e))?;
    FsRoot::new(root, Some(prefix)).map_err(|e| unavailable(&e))
}

impl ManifestSource for HostSource {
    async fn manifest(
        &self,
        path: &str,
        recursion: FileRecursion,
        hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpError> {
        let fs = self.fs()?;
        let path = path.to_string();
        tokio::task::spawn_blocking(move || fs.manifest(&path, recursion, hashing))
            .await
            .unwrap_or_else(|e| {
                Err(FileOpError::failed(
                    FileOpErrorKind::Io,
                    format!("file tracking task failed: {e}"),
                ))
            })
    }
}

fn mutates(op: &FileOp) -> bool {
    matches!(
        op,
        FileOp::Write { .. }
            | FileOp::Edit { .. }
            | FileOp::Remove { .. }
            | FileOp::Mkdir { .. }
            | FileOp::Rename { .. }
            | FileOp::ArchivePut { .. }
    )
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn leases(&self) -> std::sync::MutexGuard<'_, HashMap<String, Workspace>> {
        self.leases.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn insert(&self, lease_id: &LeaseId, root: PathBuf, readonly: bool) {
        // The tree as it stands now is what `changes` without a checkpoint compares against.
        // A workspace too large to track has no such baseline and `changes` asks for a
        // checkpoint instead.
        let baseline = HostSource { root: root.clone() }.fs().and_then(|fs| {
            fs.manifest(
                SANDBOX_WORKSPACE_PATH,
                FileRecursion::Recursive,
                FileContentHashing::default(),
            )
        });
        match baseline {
            Ok(manifest) => {
                self.tracking
                    .set_start_baseline(lease_id.as_str(), SANDBOX_WORKSPACE_PATH, manifest);
            }
            Err(_) => self.tracking.forget(lease_id.as_str()),
        }
        self.leases().insert(lease_id.to_string(), Workspace { root, readonly });
    }

    pub fn remove(&self, lease_id: &LeaseId) {
        self.leases().remove(lease_id.as_str());
        self.tracking.forget(lease_id.as_str());
    }

    /// Watch overflows recorded since the last call, as evidence subjects and records.
    pub fn take_overflow_evidence(
        &self,
        lease_id: &LeaseId,
    ) -> Vec<(String, trogon_sandbox_contract::FileWatchOverflowEvidence)> {
        self.tracking.take_overflow_evidence(lease_id.as_str(), lease_id)
    }

    pub fn contains(&self, lease_id: &LeaseId) -> bool {
        self.leases().contains_key(lease_id.as_str())
    }

    /// Runs the operation for a lease this worker owns. `None` means the lease is not
    /// here, and the caller stays silent so the worker that owns it answers.
    pub async fn execute(&self, request: &FileOpRequest) -> Option<FileOpResponse> {
        let workspace = self.leases().get(request.lease_id.as_str()).cloned()?;
        if request.as_user.is_some() {
            return Some(FileOpResponse::Unsupported {
                message: "the subprocess provider has no per-lease users, so a file operation cannot run as one"
                    .to_string(),
            });
        }
        if workspace.readonly && mutates(&request.op) {
            return Some(FileOpResponse::Failed {
                kind: FileOpErrorKind::PermissionDenied,
                message: "the workspace is mounted read-only".to_string(),
            });
        }
        let source = HostSource {
            root: workspace.root.clone(),
        };
        if let Some(reply) = self
            .tracking
            .serve(request.lease_id.as_str(), &request.op, &source)
            .await
        {
            return Some(reply);
        }
        let op = request.op.clone();
        let outcome = tokio::task::spawn_blocking(move || match workspace_fs(&workspace.root) {
            Ok(root) => root.execute(&op),
            Err(e) => e.into(),
        })
        .await;
        Some(outcome.unwrap_or_else(|e| FileOpResponse::Failed {
            kind: FileOpErrorKind::Io,
            message: format!("file operation task failed: {e}"),
        }))
    }
}

#[cfg(test)]
mod tests;
