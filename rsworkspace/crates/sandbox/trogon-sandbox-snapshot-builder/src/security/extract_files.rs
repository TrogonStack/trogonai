use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Instant;

use super::ExtractError;

#[cfg(unix)]
pub(super) fn set_mode(path: &Path, mode: u32) -> Result<(), ExtractError> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn set_mode(_path: &Path, _mode: u32) -> Result<(), ExtractError> {
    Err(ExtractError::Rejected(
        "Firecracker extraction requires Unix resource limits",
    ))
}

pub(super) fn create_file(path: &Path) -> Result<File, ExtractError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    Ok(options.open(path)?)
}

pub(super) fn check_deadline(deadline: Instant) -> Result<(), ExtractError> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(ExtractError::DeadlineExceeded)
    }
}
