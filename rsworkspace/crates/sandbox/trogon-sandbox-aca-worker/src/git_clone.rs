use std::path::Path;

use tokio::process::Command;
use trogon_sandbox_contract::{CloneAuth, SecretRef};

use crate::AcaError;

fn resolve_secret_literal(secret_ref: &SecretRef) -> Result<&str, AcaError> {
    match secret_ref {
        SecretRef::Literal { value } => Ok(value),
        SecretRef::NatsKv { .. } => Err(AcaError::Workspace(
            "NATS KV secret refs are not resolved in the ACA worker yet".to_owned(),
        )),
    }
}

/// The clone URL with the credentials of `auth` embedded in it.
pub fn authenticated_clone_url(url: &str, auth: &CloneAuth) -> Result<String, AcaError> {
    let (user, secret_ref) = match auth {
        CloneAuth::None(_) => return Ok(url.to_owned()),
        CloneAuth::BearerToken { token_ref } => ("oauth2", token_ref),
        CloneAuth::Basic { username, password_ref } => (username.as_str(), password_ref),
    };
    let secret = resolve_secret_literal(secret_ref)?;
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| AcaError::Workspace("clone_url must contain ://".to_owned()))?;
    Ok(format!("{scheme}://{user}:{secret}@{rest}"))
}

/// Clones `url` shallowly into `dest` and checks out `sha`.
pub async fn clone_git_workspace(url: &str, sha: &str, auth: &CloneAuth, dest: &Path) -> Result<(), AcaError> {
    let clone_url = authenticated_clone_url(url, auth)?;
    let cloned = Command::new("git")
        .args(["clone", "--depth=1", &clone_url])
        .arg(dest)
        .output()
        .await?;
    if !cloned.status.success() {
        return Err(AcaError::Workspace(format!(
            "git clone failed: {}",
            String::from_utf8_lossy(&cloned.stderr)
        )));
    }
    let checked_out = Command::new("git")
        .arg("-C")
        .arg(dest)
        .args(["checkout", sha])
        .output()
        .await?;
    if !checked_out.status.success() {
        return Err(AcaError::Workspace(format!(
            "git checkout failed: {}",
            String::from_utf8_lossy(&checked_out.stderr)
        )));
    }
    Ok(())
}
