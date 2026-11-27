use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use trogon_sandbox_contract::{CloneAuth, LeaseId, LeaseRequest, SandboxImage, SecretRef, WorkspaceSource};
use trogon_sandbox_protocol::SnapshotError;

use crate::constants::GIT_STDERR_CAP;

pub struct MaterializedWorkspace {
    pub host_path: PathBuf,
    pub readonly: bool,
    pub(crate) _scratch: Option<tempfile::TempDir>,
}

pub fn resolve_agent_binary(default_binary: &Path, image: &SandboxImage) -> anyhow::Result<PathBuf> {
    if image.is_unspecified() {
        return validate_executable(default_binary);
    }
    match image {
        SandboxImage::Registry { reference, .. } => {
            let path = if reference.registry.is_empty() {
                PathBuf::from(&reference.repository)
            } else {
                PathBuf::from(reference.allowlist_string())
            };
            validate_executable(&path)
        }
        SandboxImage::Snapshot { .. }
        | SandboxImage::SnapshotTag { .. }
        | SandboxImage::NativeSnapshot { .. }
        | SandboxImage::HyperlightModule { .. } => Err(anyhow::Error::new(SnapshotError::Unsupported)),
    }
}

fn validate_executable(path: &Path) -> anyhow::Result<PathBuf> {
    let resolved = path
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("agent binary `{}`: {e}", path.display()))?;
    let meta = std::fs::metadata(&resolved).map_err(|e| anyhow::anyhow!("agent binary `{}`: {e}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!("agent binary `{}` is not a regular file", path.display());
    }
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 == 0 {
        anyhow::bail!("agent binary `{}` is not executable", path.display());
    }
    Ok(resolved)
}

pub fn agent_env(req: &LeaseRequest, lease_id: &LeaseId, workspace: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "SANDBOX_WORKSPACE".to_string(),
            workspace.to_string_lossy().into_owned(),
        ),
        (
            "SANDBOX_EVIDENCE_ROOT".to_string(),
            workspace.join("evidence").to_string_lossy().into_owned(),
        ),
        ("SANDBOX_LEASE_ID".to_string(), lease_id.to_string()),
        ("SANDBOX_TENANT".to_string(), req.tenant.to_string()),
    ];
    for (key, value) in req.spec.env() {
        env.push((key.clone(), value.clone()));
    }
    env
}

pub async fn materialize_workspace(req: &LeaseRequest, workspace_root: &Path) -> anyhow::Result<MaterializedWorkspace> {
    std::fs::create_dir_all(workspace_root)
        .map_err(|e| anyhow::anyhow!("failed to create workspace root `{}`: {e}", workspace_root.display()))?;

    match req.spec.workspace() {
        WorkspaceSource::Clone { url, sha, auth, .. } => {
            let scratch = tempfile::TempDir::new_in(workspace_root)?;
            clone_git_workspace(url, sha, auth, scratch.path()).await?;
            Ok(MaterializedWorkspace {
                host_path: scratch.path().to_path_buf(),
                readonly: false,
                _scratch: Some(scratch),
            })
        }
        WorkspaceSource::HostMount { path, access } => {
            if path.is_empty() {
                anyhow::bail!("host_mount path must not be empty");
            }
            let host = PathBuf::from(path);
            if !host.is_absolute() {
                anyhow::bail!("host_mount path must be absolute: {path}");
            }
            if !host.exists() {
                anyhow::bail!("host_mount path does not exist: {path}");
            }
            Ok(MaterializedWorkspace {
                host_path: host,
                readonly: access.is_read_only(),
                _scratch: None,
            })
        }
        WorkspaceSource::Empty(_) => {
            let scratch = tempfile::TempDir::new_in(workspace_root)?;
            Ok(MaterializedWorkspace {
                host_path: scratch.path().to_path_buf(),
                readonly: false,
                _scratch: Some(scratch),
            })
        }
        WorkspaceSource::S3Volume { .. } => {
            anyhow::bail!("s3_volume workspaces are not supported on the subprocess provider")
        }
    }
}

async fn clone_git_workspace(url: &str, sha: &str, auth: &CloneAuth, dest: &Path) -> anyhow::Result<()> {
    let clone_url = build_authenticated_clone_url(url, auth)?;

    let (success, stderr) = run_git(&["clone", "--depth=1", &clone_url, &dest.to_string_lossy()]).await?;
    anyhow::ensure!(success, "git clone failed: {stderr}");

    let (success, stderr) = run_git(&["-C", &dest.to_string_lossy(), "checkout", sha]).await?;
    anyhow::ensure!(success, "git checkout failed: {stderr}");

    Ok(())
}

/// Runs git, discarding stdout and keeping a bounded head and tail of stderr,
/// so a noisy remote cannot grow the worker's memory.
async fn run_git(args: &[&str]) -> anyhow::Result<(bool, String)> {
    let mut child = tokio::process::Command::new("git")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("git stderr was not captured"))?;
    let mut kept = trogon_sandbox_protocol::CappedOutput::new(GIT_STDERR_CAP);
    let mut chunk = [0u8; 8192];
    loop {
        match stderr.read(&mut chunk).await? {
            0 => break,
            read => kept.push(&chunk[..read]),
        }
    }
    let status = child.wait().await?;
    Ok((
        status.success(),
        String::from_utf8_lossy(&kept.into_bytes()).into_owned(),
    ))
}

fn build_authenticated_clone_url(url: &str, auth: &CloneAuth) -> anyhow::Result<String> {
    let token = match auth {
        CloneAuth::None(_) => None,
        CloneAuth::BearerToken { token_ref } => Some(resolve_secret_literal(token_ref)?),
        CloneAuth::Basic { password_ref, username } => {
            let password = resolve_secret_literal(password_ref)?;
            let sep = url
                .find("://")
                .ok_or_else(|| anyhow::anyhow!("clone_url must contain ://"))?;
            let scheme = &url[..sep + 3];
            let rest = &url[sep + 3..];
            return Ok(format!("{scheme}{username}:{password}@{rest}"));
        }
    };

    if let Some(tok) = token {
        let sep = url
            .find("://")
            .ok_or_else(|| anyhow::anyhow!("clone_url must contain ://"))?;
        let scheme = &url[..sep + 3];
        let rest = &url[sep + 3..];
        Ok(format!("{scheme}oauth2:{tok}@{rest}"))
    } else {
        Ok(url.to_string())
    }
}

fn resolve_secret_literal(secret: &SecretRef) -> anyhow::Result<String> {
    match secret {
        SecretRef::Literal { value } => Ok(value.clone()),
        SecretRef::NatsKv { .. } => {
            anyhow::bail!("NatsKv SecretRef is not supported in subprocess worker; use Literal for local testing")
        }
    }
}
