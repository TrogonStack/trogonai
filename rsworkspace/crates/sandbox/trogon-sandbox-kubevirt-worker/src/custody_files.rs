use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::CustodyError;
use crate::constants::CUSTODY_MAX_BYTES;
#[cfg(unix)]
use crate::constants::{CUSTODY_DIR_MODE, CUSTODY_FILE_MODE};

/// Fails with `error` unless `condition` holds.
pub(crate) fn require(condition: bool, error: CustodyError) -> Result<(), CustodyError> {
    if condition { Ok(()) } else { Err(error) }
}

/// Reads a regular custody file within its size budget and parses it.
pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, CustodyError> {
    let metadata = std::fs::symlink_metadata(path)?;
    require(
        metadata.is_file() && metadata.len() <= CUSTODY_MAX_BYTES,
        CustodyError::InvalidFile,
    )?;
    let mut bytes = Vec::new();
    File::open(path)?.take(CUSTODY_MAX_BYTES + 1).read_to_end(&mut bytes)?;
    let within_budget = u64::try_from(bytes.len()).is_ok_and(|length| length <= CUSTODY_MAX_BYTES);
    require(within_budget, CustodyError::RecordTooLarge)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Options that open `path` for reading and writing with owner-only permissions, refusing anything that is not
/// a regular file.
pub(crate) fn private_file(path: &Path) -> Result<OpenOptions, CustodyError> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        require(metadata.is_file(), CustodyError::InvalidFile)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, CUSTODY_FILE_MODE);
    Ok(options)
}

/// Restricts the custody directory to its owner.
#[cfg(unix)]
pub(crate) fn private_directory(path: &Path) -> Result<(), CustodyError> {
    let permissions = std::os::unix::fs::PermissionsExt::from_mode(CUSTODY_DIR_MODE);
    Ok(std::fs::set_permissions(path, permissions)?)
}

/// Restricts the custody directory to its owner.
#[cfg(not(unix))]
pub(crate) fn private_directory(_path: &Path) -> Result<(), CustodyError> {
    Ok(())
}

/// Writes `value` to `path` atomically: a synced sibling is renamed over it and the directory is synced.
pub(crate) fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), CustodyError> {
    let bytes = serde_json::to_vec(value)?;
    let within_budget = u64::try_from(bytes.len()).is_ok_and(|length| length <= CUSTODY_MAX_BYTES);
    require(within_budget, CustodyError::RecordTooLarge)?;
    let pending = path.with_extension("pending");
    let mut options = private_file(&pending)?;
    options.truncate(true);
    let mut file = options.open(&pending)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&pending, path)?;
    sync_parent(path)
}

/// Makes the directory entries under `directory` durable.
pub(crate) fn sync_directory(directory: &Path) -> Result<(), CustodyError> {
    Ok(File::open(directory)?.sync_all()?)
}

fn sync_parent(path: &Path) -> Result<(), CustodyError> {
    path.parent().map_or(Err(CustodyError::InvalidFile), sync_directory)
}
