use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use super::mount_plan::{GuestLayout, MountAccess, root_path};
#[cfg(target_os = "linux")]
use anyhow::Context;

/// The sandbox sees the host's own filesystem at the host's own paths, with
/// the command's working directory where the caller is. Nothing is writable
/// until the filesystem policy grants it, and the environment is exactly the
/// one in the lease.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HostView {
    cwd: PathBuf,
}

impl HostView {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

#[cfg(target_os = "linux")]
pub(super) fn host_layout() -> anyhow::Result<GuestLayout> {
    let root = std::fs::canonicalize("/").context("resolving the host root")?;
    let mut layout = GuestLayout::host_view();
    layout.bind(root_path()?, root, MountAccess::ReadWrite);
    Ok(layout)
}
