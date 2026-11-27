use std::path::Path;

use trogon_sandbox_contract::ShortLivedToken;
use walkdir::WalkDir;

use crate::{AcaError, AcaSessionApi, SessionIdentifier};

/// Uploads every file under `root` to the session, keyed by its path relative to `root`, and
/// returns how many files it uploaded.
pub async fn upload_directory(
    api: &dyn AcaSessionApi,
    bearer: &ShortLivedToken,
    identifier: &SessionIdentifier,
    root: &Path,
) -> Result<u64, AcaError> {
    let mut files = 0u64;
    for entry in WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let relative = entry.path().strip_prefix(root).unwrap_or_else(|_| entry.path());
        let remote = relative.to_string_lossy().replace('\\', "/");
        let data = tokio::fs::read(entry.path()).await?;
        api.upload_file(bearer, identifier, &remote, &data).await?;
        files += 1;
    }
    Ok(files)
}
