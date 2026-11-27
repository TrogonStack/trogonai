//! The per-lease directory: everything one microVM owns on the host lives under it.

use std::io;
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::LeaseId;

use crate::config::StateDir;

#[derive(Debug, thiserror::Error)]
pub enum VmDirError {
    #[error("lease id {0:?} cannot name a directory: only ASCII letters, digits, '-' and '_' are allowed")]
    InvalidLeaseId(String),
    #[error("jailer executable must have a plain filename: {0:?}")]
    InvalidJailerExecutable(String),
    #[error(
        "socket path {path} is {len} bytes, over the {MAX_SOCKET_PATH_BYTES} byte limit for Unix sockets; use a shorter state dir"
    )]
    SocketPathTooLong { path: PathBuf, len: usize },
    #[error("lease directory {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

/// `<state_dir>/<lease_id>/`, the only place a lease's VM writes to.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VmDir {
    lease: PathBuf,
    runtime: PathBuf,
}

impl VmDir {
    pub fn for_lease(state_dir: &StateDir, lease_id: &LeaseId) -> Result<Self, VmDirError> {
        let id = lease_id.as_str();
        let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(VmDirError::InvalidLeaseId(id.to_string()));
        }
        let state = if state_dir.as_path().is_absolute() {
            state_dir.as_path().to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|source| VmDirError::Io {
                    path: state_dir.as_path().to_path_buf(),
                    source,
                })?
                .join(state_dir.as_path())
        };
        let lease = state.join(id);
        let dir = Self {
            runtime: lease.clone(),
            lease,
        };
        dir.validate_sockets()?;
        Ok(dir)
    }

    pub fn jailed(state_dir: &StateDir, lease_id: &LeaseId, executable: &str) -> Result<Self, VmDirError> {
        if !matches!(
            Path::new(executable).components().collect::<Vec<_>>().as_slice(),
            [std::path::Component::Normal(_)]
        ) {
            return Err(VmDirError::InvalidJailerExecutable(executable.into()));
        }
        let mut dir = Self::for_lease(state_dir, lease_id)?;
        dir.runtime = dir.lease.join("jail").join(executable).join("vm").join("root");
        dir.validate_sockets()?;
        Ok(dir)
    }

    fn validate_sockets(&self) -> Result<(), VmDirError> {
        for socket in [self.api_socket(), self.vsock_socket()] {
            let len = socket.as_os_str().len();
            if len > MAX_SOCKET_PATH_BYTES {
                return Err(VmDirError::SocketPathTooLong { path: socket, len });
            }
        }
        Ok(())
    }

    pub fn as_path(&self) -> &Path {
        &self.lease
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime
    }

    /// The lease a directory was created for, recovered from its name.
    pub fn lease_id(&self) -> Option<LeaseId> {
        self.lease
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| LeaseId::new(name.to_string()))
    }

    pub fn api_socket(&self) -> PathBuf {
        self.runtime.join(API_SOCKET)
    }

    pub fn vsock_socket(&self) -> PathBuf {
        self.runtime.join(VSOCK_SOCKET)
    }

    pub fn rootfs(&self) -> PathBuf {
        self.runtime.join(ROOTFS)
    }

    /// The VMM runs inside the lease directory and is given device paths relative to it, so
    /// a snapshot of one lease restores against the files of whichever directory loads it.
    pub fn rootfs_from_vmm() -> PathBuf {
        PathBuf::from(ROOTFS)
    }

    pub fn vsock_socket_from_vmm() -> PathBuf {
        PathBuf::from(VSOCK_SOCKET)
    }

    pub fn log_file(&self) -> PathBuf {
        self.lease.join(LOG)
    }

    pub fn pid_file(&self) -> PathBuf {
        self.lease.join(PID_FILE)
    }

    /// Creates the directory, tolerating one left behind by a failed earlier attempt.
    pub async fn create(&self) -> Result<(), VmDirError> {
        tokio::fs::create_dir_all(&self.runtime)
            .await
            .map_err(|source| self.io_error(source))
    }

    /// Removes the directory and everything in it. Removing one that is already gone is not
    /// an error, so teardown can run any number of times.
    pub async fn remove(&self) -> Result<(), VmDirError> {
        match tokio::fs::remove_dir_all(&self.lease).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(self.io_error(source)),
        }
    }

    fn io_error(&self, source: io::Error) -> VmDirError {
        VmDirError::Io {
            path: self.lease.clone(),
            source,
        }
    }
}

/// The lease directories a previous run of this worker left under `state_dir`.
pub async fn stale_dirs(state_dir: &StateDir) -> io::Result<Vec<VmDir>> {
    let mut entries = match tokio::fs::read_dir(state_dir.as_path()).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut dirs = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name() == RETAINED_DIRECTORY {
            continue;
        }
        if entry.file_type().await?.is_dir() {
            dirs.push(VmDir {
                lease: entry.path(),
                runtime: entry.path(),
            });
        }
    }
    Ok(dirs)
}

#[cfg(test)]
mod tests;

mod constants;
pub use constants::RETAINED_DIRECTORY;
use constants::{API_SOCKET, LOG, MAX_SOCKET_PATH_BYTES, PID_FILE, ROOTFS, VSOCK_SOCKET};
