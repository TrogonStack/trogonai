use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use trogon_sandbox_contract::{
    LeaseError, ReadFileRequest, ReadFileResponse, SandboxFileWrite, WriteFilesRequest, WriteFilesResponse,
};

use crate::LeaseBinding;
use crate::lease_fault::backend;
use crate::shell_quote::shell_quote;
use crate::target_user::TargetUser;

/// The shell a file operation borrows when it has to run as a sandbox user.
const SHELL: &str = "/bin/sh";

/// Writes files into the actor's own filesystem.
///
/// Relative paths resolve against the lease's bound working directory, matching how a relative `cwd` behaves for exec,
/// so a caller that never sets `working_dir` still gets the process's own working directory rather than an arbitrary
/// one.
///
/// `as_user` is not advisory. The write is performed by a child process that has dropped to that user, so the kernel
/// applies the same permission checks it would to a command the tenant ran themselves; doing the write as root and
/// chowning afterwards would let a caller reach files the user they named could never touch.
pub async fn write_files(request: &WriteFilesRequest, binding: &LeaseBinding) -> WriteFilesResponse {
    let user = match request.as_user.as_ref().map(TargetUser::resolve).transpose() {
        Ok(user) => user,
        Err(error) => return WriteFilesResponse::Denied(error),
    };
    let base = base_dir(user.as_ref(), binding);

    for file in &request.files {
        if let Err(error) = write_one(file, base.as_deref(), user.as_ref()).await {
            return WriteFilesResponse::Denied(error);
        }
    }
    WriteFilesResponse::Written
}

pub async fn read_file(request: &ReadFileRequest, binding: &LeaseBinding) -> ReadFileResponse {
    let user = match request.as_user.as_ref().map(TargetUser::resolve).transpose() {
        Ok(user) => user,
        Err(error) => return ReadFileResponse::Denied(error),
    };
    let path = resolve(&request.path, base_dir(user.as_ref(), binding).as_deref());

    match user.as_ref() {
        Some(user) => read_as_user(SHELL, &path, user).await,
        None => read_directly(&path).await,
    }
}

async fn write_one(file: &SandboxFileWrite, base: Option<&str>, user: Option<&TargetUser>) -> Result<(), LeaseError> {
    let path = resolve(&file.path, base);
    match user {
        Some(user) => write_as_user(SHELL, &path, &file.content, file.mode, user).await,
        None => write_directly(&path, &file.content, file.mode).await,
    }
}

async fn read_directly(path: &Path) -> ReadFileResponse {
    match tokio::fs::read(path).await {
        Ok(data) => ReadFileResponse::Content { data },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReadFileResponse::NotFound,
        Err(error) => ReadFileResponse::Denied(backend(format!("failed to read {}: {error}", path.display()))),
    }
}

async fn write_directly(path: &Path, content: &[u8], mode: Option<u32>) -> Result<(), LeaseError> {
    // A bare filename has an empty parent, which is the working directory and needs no creating.
    let parent = path.parent().unwrap_or(Path::new(""));
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| backend(format!("failed to create {}: {e}", parent.display())))?;
    tokio::fs::write(path, content)
        .await
        .map_err(|e| backend(format!("failed to write {}: {e}", path.display())))?;
    match mode {
        Some(mode) => tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .await
            .map_err(|e| backend(format!("failed to chmod {}: {e}", path.display()))),
        None => Ok(()),
    }
}

async fn write_as_user(
    shell: &str,
    path: &Path,
    content: &[u8],
    mode: Option<u32>,
    user: &TargetUser,
) -> Result<(), LeaseError> {
    // The content travels on stdin rather than inside the script, so a file that happens to contain a quote, a newline
    // or a NUL is written byte for byte.
    let quoted = shell_quote(&path.to_string_lossy());
    let mut script = String::from("set -e\n");
    let parent = path.parent().unwrap_or(Path::new(""));
    script.push_str(&format!("mkdir -p {}\n", shell_quote(&parent.to_string_lossy())));
    script.push_str(&format!("cat > {quoted}\n"));
    if let Some(mode) = mode {
        script.push_str(&format!("chmod {mode:o} {quoted}\n"));
    }

    let mut child = spawn_as_user(shell, &script, user, Stdio::piped())?;
    for mut stdin in child.stdin.take().into_iter() {
        // A failed write is not reported here: the shell may have already exited, and the exit status below says why
        // far more precisely than a broken pipe would.
        let _ = stdin.write_all(content).await;
        let _ = stdin.shutdown().await;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| backend(format!("failed to write {}: {e}", path.display())))?;

    if output.status.success() {
        return Ok(());
    }
    Err(backend(format!(
        "failed to write {} as {}: {}",
        path.display(),
        user.name(),
        stderr_of(&output.stderr)
    )))
}

async fn read_as_user(shell: &str, path: &Path, user: &TargetUser) -> ReadFileResponse {
    let output = match capture_as_user(shell, path, user).await {
        Ok(output) => output,
        Err(error) => return ReadFileResponse::Denied(error),
    };

    if output.status.success() {
        return ReadFileResponse::Content { data: output.stdout };
    }
    let stderr = stderr_of(&output.stderr);
    // Only a path that does not exist is "not found": a file that exists but belongs to another user is exactly what
    // asking for `as_user` is meant to surface as a denial, and calling it missing would be a lie.
    if stderr.contains("No such file or directory") {
        return ReadFileResponse::NotFound;
    }
    ReadFileResponse::Denied(backend(format!(
        "failed to read {} as {}: {stderr}",
        path.display(),
        user.name()
    )))
}

async fn capture_as_user(shell: &str, path: &Path, user: &TargetUser) -> Result<Output, LeaseError> {
    let script = format!("cat -- {}", shell_quote(&path.to_string_lossy()));
    let child = spawn_as_user(shell, &script, user, Stdio::null())?;
    child
        .wait_with_output()
        .await
        .map_err(|e| backend(format!("failed to read {}: {e}", path.display())))
}

fn spawn_as_user(shell: &str, script: &str, user: &TargetUser, stdin: Stdio) -> Result<Child, LeaseError> {
    let mut command = Command::new(shell);
    command.arg("-c").arg(script);
    command.stdin(stdin);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    user.apply(&mut command);
    command
        .spawn()
        .map_err(|e| backend(format!("failed to start {shell} as {}: {e}", user.name())))
}

fn stderr_of(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).trim().chars().take(512).collect()
}

/// Where a relative path resolves.
///
/// A sandbox user's home wins over the lease's working directory, mirroring how an exec run as that user starts in
/// their home, so a write and a subsequent command agree on what a bare filename means.
fn base_dir(user: Option<&TargetUser>, binding: &LeaseBinding) -> Option<String> {
    user.map(|user| user.home().to_string())
        .or_else(|| binding.working_dir.clone())
}

fn resolve(path: &str, base: Option<&str>) -> PathBuf {
    let path = Path::new(path);
    match base {
        Some(base) => Path::new(base).join(path),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests;
