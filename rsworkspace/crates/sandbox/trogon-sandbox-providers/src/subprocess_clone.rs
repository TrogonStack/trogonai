use std::path::Path;

use trogon_sandbox_contract::{CloneAuth, SecretRef};

use crate::git_command::GitCommand;
use crate::{GitStep, SandboxSpec, SubprocessError, try_clone_with_volume_cache};

pub(crate) async fn clone_workspace_subprocess(
    git: &GitCommand,
    spec: &SandboxSpec,
    workspace_path: &str,
) -> Result<(), SubprocessError> {
    try_clone_with_volume_cache(
        spec,
        Path::new(workspace_path),
        clone_workspace_subprocess_git(git, spec, workspace_path),
    )
    .await
}

pub(crate) async fn clone_workspace_subprocess_git(
    git: &GitCommand,
    spec: &SandboxSpec,
    workspace_path: &str,
) -> Result<(), SubprocessError> {
    let Some(credentials) = &spec.clone else {
        return Ok(());
    };

    let token = match &credentials.auth {
        CloneAuth::None(_) => None,
        CloneAuth::BearerToken {
            token_ref: SecretRef::Literal { value },
        }
        | CloneAuth::Basic {
            password_ref: SecretRef::Literal { value },
            ..
        } => Some(value.as_str()),
        CloneAuth::BearerToken {
            token_ref: SecretRef::NatsKv { .. },
        }
        | CloneAuth::Basic {
            password_ref: SecretRef::NatsKv { .. },
            ..
        } => return Err(SubprocessError::NatsKvSecretUnsupported),
    };

    let clone_url = match token {
        Some(token) => {
            let (scheme, rest) = credentials
                .clone_url
                .split_once("://")
                .ok_or(SubprocessError::CloneUrlMissingScheme)?;
            format!("{scheme}://oauth2:{token}@{rest}")
        }
        None => credentials.clone_url.clone(),
    };

    let sha = spec.resolved_sha.as_str();

    run_git(
        git,
        GitStep::Clone,
        &["clone", "--depth=1", "--filter=blob:none", &clone_url, workspace_path],
    )
    .await?;
    run_git(
        git,
        GitStep::Fetch,
        &["-C", workspace_path, "fetch", "--depth=1", "origin", sha],
    )
    .await?;
    run_git(git, GitStep::Checkout, &["-C", workspace_path, "checkout", sha]).await
}

async fn run_git(git: &GitCommand, step: GitStep, args: &[&str]) -> Result<(), SubprocessError> {
    let output = git
        .command()
        .args(args)
        .output()
        .await
        .map_err(|source| SubprocessError::GitUnavailable { step, source })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SubprocessError::GitFailed {
            step,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests;
