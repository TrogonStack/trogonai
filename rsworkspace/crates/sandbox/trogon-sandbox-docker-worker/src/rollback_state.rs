//! Durable copy of the lease request an in-place rollback rebuilds from.
//!
//! The worker keeps each lease's request in memory so a rollback can recreate
//! the container with the same shape. A restart loses that, and the startup
//! sweep removes the containers anyway, so a rollback after a restart would
//! find nothing to roll back. With a state directory the request is also kept
//! on disk, one file per lease, and the rollback falls back to it when the
//! lease is no longer active: the container is gone, so it is created again
//! from the snapshot under the same lease id.
//!
//! The request can carry `env` values, so the files are written `0600` inside
//! a `0700` directory, removed when the lease is cancelled, and removed on
//! startup once older than the retention. The store is off unless the
//! operator turns it on.
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::warn;
use trogon_sandbox_contract::{LeaseId, LeaseRequest};

mod constants;

use constants::SUBDIR;

#[derive(Debug, Clone, Default)]
pub struct RollbackStateStore {
    dir: Option<PathBuf>,
}

impl RollbackStateStore {
    /// A store that remembers nothing.
    pub fn disabled() -> Self {
        Self { dir: None }
    }

    /// Keeps requests under `state_dir/rollback`, dropping entries not touched
    /// within `retention`.
    pub fn open(state_dir: &Path, retention: Duration) -> std::io::Result<Self> {
        let dir = state_dir.join(SUBDIR);
        create_private_dir(&dir)?;
        let store = Self { dir: Some(dir) };
        store.prune(retention);
        Ok(store)
    }

    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    fn path(&self, lease_id: &LeaseId) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let name: String = lease_id.as_str().bytes().map(|byte| format!("{byte:02x}")).collect();
        Some(dir.join(format!("{name}.json")))
    }

    /// Records the request. A failure is logged and does not fail the lease:
    /// the in-memory copy still serves a rollback until the next restart.
    pub async fn remember(&self, lease_id: &LeaseId, request: &LeaseRequest) {
        let Some(path) = self.path(lease_id) else {
            return;
        };
        let bytes = match serde_json::to_vec(request) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(lease_id = %lease_id, error = %e, "rollback state: serialize failed");
                return;
            }
        };
        let result = tokio::task::spawn_blocking(move || write_private(&path, &bytes)).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(lease_id = %lease_id, error = %e, "rollback state: write failed")
            }
            Err(e) => warn!(lease_id = %lease_id, error = %e, "rollback state: write aborted"),
        }
    }

    /// The remembered request, or `None` when there is none or it is unreadable.
    pub async fn load(&self, lease_id: &LeaseId) -> Option<LeaseRequest> {
        let path = self.path(lease_id)?;
        let bytes = tokio::fs::read(&path).await.ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(request) => Some(request),
            Err(e) => {
                warn!(lease_id = %lease_id, error = %e, "rollback state: unreadable entry");
                None
            }
        }
    }

    pub async fn forget(&self, lease_id: &LeaseId) {
        let Some(path) = self.path(lease_id) else {
            return;
        };
        if let Err(e) = tokio::fs::remove_file(&path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            warn!(lease_id = %lease_id, error = %e, "rollback state: remove failed");
        }
    }

    /// Removes entries last written longer ago than `retention`.
    fn prune(&self, retention: Duration) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|modified| now.duration_since(modified).unwrap_or_default() > retention)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests;
