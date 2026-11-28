use std::path::{Path, PathBuf};

use trogon_sandbox_contract::SandboxId;
use walkdir::WalkDir;

use crate::{ModalApiError, ModalSandboxApi};

/// Every regular file under `root`. A symlink is refused with its path, because uploading it as a file or
/// leaving it out would stage a different tree than the caller supplied. A directory or entry that cannot be read fails the walk, because staging a
/// workspace that silently lacks files would run the sandbox against a different tree than the caller supplied.
fn files_under(root: &Path) -> Result<Vec<PathBuf>, ModalApiError> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root) {
        let entry = entry.map_err(std::io::Error::from)?;
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            return Err(ModalApiError::SymlinkInUpload(entry.into_path()));
        }
        if file_type.is_file() {
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

/// Stages every file under `root` into the sandbox beneath `remote_root` and returns how many it wrote.
pub(crate) async fn upload_directory(
    api: &dyn ModalSandboxApi,
    sandbox_id: &SandboxId,
    remote_root: &str,
    root: &Path,
) -> Result<u64, ModalApiError> {
    let mut files = 0u64;
    for path in files_under(root)? {
        let relative = path.strip_prefix(root)?;
        let remote = format!(
            "{}/{}",
            remote_root.trim_end_matches('/'),
            relative.to_string_lossy().replace('\\', "/")
        );
        let data = tokio::fs::read(&path).await?;
        api.write_file(sandbox_id, &remote, &data).await?;
        files += 1;
    }
    Ok(files)
}

#[cfg(test)]
mod tests;
