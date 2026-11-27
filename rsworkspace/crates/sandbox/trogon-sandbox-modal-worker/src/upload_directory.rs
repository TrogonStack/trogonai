use std::path::Path;

use trogon_sandbox_contract::SandboxId;
use walkdir::WalkDir;

use crate::{ModalApiError, ModalSandboxApi};

/// Stages every file under `root` into the sandbox beneath `remote_root` and returns how many it wrote.
pub(crate) async fn upload_directory(
    api: &dyn ModalSandboxApi,
    sandbox_id: &SandboxId,
    remote_root: &str,
    root: &Path,
) -> Result<u64, ModalApiError> {
    let mut files = 0u64;
    let entries = WalkDir::new(root).into_iter().filter_map(Result::ok);
    for entry in entries.filter(|entry| entry.file_type().is_file()) {
        let relative = entry.path().strip_prefix(root)?;
        let remote = format!(
            "{}/{}",
            remote_root.trim_end_matches('/'),
            relative.to_string_lossy().replace('\\', "/")
        );
        let data = tokio::fs::read(entry.path()).await?;
        api.write_file(sandbox_id, &remote, &data).await?;
        files += 1;
    }
    Ok(files)
}
