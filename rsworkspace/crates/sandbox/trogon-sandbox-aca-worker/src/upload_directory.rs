use std::path::Path;

use trogon_sandbox_contract::ShortLivedToken;
use walkdir::WalkDir;

use crate::{AcaError, AcaSessionApi, SessionIdentifier};

/// Uploads every file under `root` to the session, keyed by its path relative to `root`, and
/// returns how many files it uploaded. An entry that cannot be read fails the upload rather than being skipped.
pub async fn upload_directory(
    api: &dyn AcaSessionApi,
    bearer: &ShortLivedToken,
    identifier: &SessionIdentifier,
    root: &Path,
) -> Result<u64, AcaError> {
    let mut files = 0u64;
    for entry in WalkDir::new(root).sort_by_file_name() {
        let entry = entry.map_err(std::io::Error::from)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root).map_err(std::io::Error::other)?;
        let remote = relative.to_string_lossy().replace('\\', "/");
        let data = tokio::fs::read(entry.path()).await?;
        api.upload_file(bearer, identifier, &remote, &data).await?;
        files += 1;
    }
    Ok(files)
}
