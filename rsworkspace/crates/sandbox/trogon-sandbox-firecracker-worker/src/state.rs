use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;

use crate::config::StateDir;
use crate::error::ProviderError;

pub(crate) struct StateDirectoryLock {
    _file: File,
}

impl Drop for StateDirectoryLock {
    fn drop(&mut self) {
        if let Err(error) = fs2::FileExt::unlock(&self._file) {
            tracing::warn!(%error, "releasing worker state directory lock failed");
        }
    }
}

impl StateDirectoryLock {
    pub(crate) async fn acquire(state: &StateDir) -> Result<Self, ProviderError> {
        let fail = |error| ProviderError::Launch(format!("locking worker state directory: {error}"));
        tokio::fs::create_dir_all(state.as_path()).await.map_err(fail)?;
        let directory = tokio::fs::canonicalize(state.as_path()).await.map_err(fail)?;
        let name = directory
            .file_name()
            .ok_or_else(|| ProviderError::Launch("worker state directory cannot be a filesystem root".into()))?;
        let mut lock_name = OsString::from(".");
        lock_name.push(name);
        lock_name.push(".sandbox-fc.lock");
        let lock_path = directory
            .parent()
            .ok_or_else(|| ProviderError::Launch("worker state directory cannot be a filesystem root".into()))?
            .join(lock_name);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)
            .map_err(fail)?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
            ProviderError::Launch(format!(
                "worker state directory {} is already owned or cannot be locked: {error}",
                directory.display()
            ))
        })?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests;
