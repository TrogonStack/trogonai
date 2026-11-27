use std::path::Path;

use tokio::process::Command;

use crate::{CloneUrl, CommitSha, WorkspaceError};

/// Clones `url` shallowly into `dest` and checks out `sha`.
///
/// `--` keeps the url from reading as an option, and `sha` is hexadecimal by type.
pub(crate) async fn clone_checkout(url: &CloneUrl, sha: &CommitSha, dest: &Path) -> Result<(), WorkspaceError> {
    git(
        "clone",
        &["clone", "--depth=1", "--", url.as_str(), &dest.to_string_lossy()],
    )
    .await?;
    git("checkout", &["-C", &dest.to_string_lossy(), "checkout", sha.as_str()]).await
}

async fn git(step: &'static str, args: &[&str]) -> Result<(), WorkspaceError> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(WorkspaceError::Git {
            step,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}
