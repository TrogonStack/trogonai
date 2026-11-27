//! Persisted owner identity for the Docker worker.
//!
//! The owner is labelled onto every container the worker creates and drives
//! the startup sweep of containers a previous run left behind, so it must
//! survive restarts. Each state directory holds exactly one owner and is
//! locked for the life of the process, so two workers cannot share one.

use std::fs::File;
use std::path::Path;

mod constants;
mod owner_store_error;

use constants::{LOCK_FILE, OWNER_FILE};
pub use owner_store_error::OwnerStoreError;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// The worker's owner id plus the lock that keeps its state directory
/// exclusive. Keep it alive for as long as the worker runs.
#[derive(Debug)]
pub struct PersistedOwner {
    pub owner: String,
    _lock: File,
}

/// Locks `state_dir` and returns the owner stored there, creating the
/// directory and a fresh owner when none is recorded. An empty or corrupt
/// record is replaced rather than trusted, since a fresh owner only means
/// the previous run's leftovers are not swept.
pub fn load_or_create(state_dir: &Path) -> Result<PersistedOwner, OwnerStoreError> {
    std::fs::create_dir_all(state_dir).map_err(|source| OwnerStoreError::CreateDir {
        path: state_dir.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state_dir.join(LOCK_FILE))?;
    lock.try_lock().map_err(OwnerStoreError::Locked)?;

    let path = state_dir.join(OWNER_FILE);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text.trim().to_string()).filter(|t| valid(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let owner = match existing {
        Some(owner) => owner,
        None => {
            let owner = crate::fresh_id::fresh_id();
            let tmp = state_dir.join(format!("{OWNER_FILE}.tmp"));
            std::fs::write(&tmp, &owner)?;
            std::fs::rename(&tmp, &path)?;
            owner
        }
    };
    Ok(PersistedOwner { owner, _lock: lock })
}

fn valid(owner: &str) -> bool {
    crate::fresh_id::is_fresh_id(owner)
}

#[cfg(test)]
mod tests;
