use std::path::Path;

use crate::{CloneCredentials, DockerError, SandboxSpec, try_clone_with_volume_cache};
use trogon_sandbox_contract::{CloneAuth, SecretRef};

/// Clones the spec's repository into `workspace_path`, through the volume cache when enabled.
pub(crate) async fn clone_workspace(
    spec: &SandboxSpec,
    creds: &CloneCredentials,
    workspace_path: &str,
) -> Result<(), DockerError> {
    let git_clone = clone_workspace_git(spec, creds, workspace_path);
    try_clone_with_volume_cache(spec, Path::new(workspace_path), git_clone).await
}

fn clone_token(auth: &CloneAuth) -> Result<Option<&str>, DockerError> {
    match auth {
        CloneAuth::None(_) => Ok(None),
        CloneAuth::BearerToken {
            token_ref: SecretRef::Literal { value },
        }
        | CloneAuth::Basic {
            password_ref: SecretRef::Literal { value },
            ..
        } => Ok(Some(value.as_str())),
        CloneAuth::BearerToken {
            token_ref: SecretRef::NatsKv { .. },
        }
        | CloneAuth::Basic {
            password_ref: SecretRef::NatsKv { .. },
            ..
        } => Err(DockerError::NatsKvSecretUnsupported),
    }
}

async fn clone_workspace_git(
    spec: &SandboxSpec,
    creds: &CloneCredentials,
    workspace_path: &str,
) -> Result<(), DockerError> {
    let clone_url = match clone_token(&creds.auth)? {
        Some(token) => inject_token_into_url(&creds.clone_url, token)?,
        None => creds.clone_url.clone(),
    };

    let sha = spec.resolved_sha.as_str();

    run_git(
        "clone",
        &["clone", "--depth=1", "--filter=blob:none", &clone_url, workspace_path],
    )
    .await?;
    run_git("fetch", &["-C", workspace_path, "fetch", "--depth=1", "origin", sha]).await?;
    run_git("checkout", &["-C", workspace_path, "checkout", sha]).await
}

async fn run_git(command: &'static str, args: &[&str]) -> Result<(), DockerError> {
    run_program("git", command, args).await
}

async fn run_program(program: &str, command: &'static str, args: &[&str]) -> Result<(), DockerError> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|source| DockerError::GitSpawn { command, source })?;
    if output.status.success() {
        return Ok(());
    }
    Err(DockerError::GitFailed {
        command,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn inject_token_into_url(url: &str, token: &str) -> Result<String, DockerError> {
    let (scheme, rest) = url.split_once("://").ok_or(DockerError::CloneUrlMissingScheme)?;
    Ok(format!("{scheme}://oauth2:{token}@{rest}"))
}

#[cfg(test)]
mod tests;
