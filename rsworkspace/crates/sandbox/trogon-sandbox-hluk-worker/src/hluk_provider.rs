use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    CommandExecution, LaunchCancellation, LeaseError, LeaseId, LeaseRequest, ProviderKind, ProviderLease,
    ReadFileRequest, ReadFileResponse, RunCommandRequest, RunCommandResponse, SandboxId, WriteFilesRequest,
    WriteFilesResponse,
};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{
    Provider, ProviderError, RedactionSet, options_for, reject_unknown_keys, reject_unsupported_privileged,
    resolve_exec_cwd,
};

use crate::constants::WORKSPACE_GUEST_DIR;
use crate::{HlukCli, HlukLease, HlukRuntime, PROFILE, WorkspaceAccess, WorkspaceOwner};

fn denied(message: impl Into<String>) -> LeaseError {
    LeaseError::Backend {
        message: message.into(),
    }
}

/// Serves leases with `hluk`: a lease is a runtime whose warm snapshot exists plus a workspace directory, and each
/// command is one `hluk run` restored from that snapshot. Nothing in the guest outlives a command except the
/// workspace.
pub struct HlukProvider {
    cli: HlukCli,
    work_root: PathBuf,
    leases: Mutex<HashMap<LeaseId, Arc<HlukLease>>>,
    /// Runtimes whose snapshot this worker has saved. Held across a warm-up so concurrent leases wait for it.
    warmed: tokio::sync::Mutex<HashSet<HlukRuntime>>,
}

impl HlukProvider {
    /// A provider running `cli`, creating lease workspaces under `work_root`.
    pub fn new(cli: HlukCli, work_root: PathBuf) -> Self {
        Self {
            cli,
            work_root,
            leases: Mutex::default(),
            warmed: tokio::sync::Mutex::default(),
        }
    }

    fn held(&self) -> MutexGuard<'_, HashMap<LeaseId, Arc<HlukLease>>> {
        self.leases.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The lease `id`, when this worker holds it.
    pub fn lease_of(&self, id: &LeaseId) -> Option<Arc<HlukLease>> {
        self.held().get(id).cloned()
    }

    /// How many leases this worker holds.
    pub fn lease_count(&self) -> usize {
        self.held().len()
    }

    async fn warm(&self, runtime: HlukRuntime) -> Result<(), ProviderError> {
        let mut warmed = self.warmed.lock().await;
        if !warmed.contains(&runtime) {
            self.cli.warm(runtime).await.map_err(ProviderError::backend)?;
            warmed.insert(runtime);
        }
        Ok(())
    }

    /// Runs `request` on `lease` to completion in a guest restored for it.
    pub async fn run_command(&self, lease: &HlukLease, request: &RunCommandRequest) -> RunCommandResponse {
        if matches!(request.execution, CommandExecution::Detached { .. }) {
            return RunCommandResponse::Denied(denied(
                "hluk restores a fresh guest per command and keeps none running: detached commands are not supported",
            ));
        }
        if request.run_as_user.is_some() {
            return RunCommandResponse::Denied(denied("hluk guests have one user; run_as_user is not supported"));
        }
        if request.require_launch_cancellation == LaunchCancellation::Required {
            return RunCommandResponse::Denied(denied("command launch cancellation is not supported by hluk"));
        }
        let cwd = resolve_exec_cwd(WORKSPACE_GUEST_DIR, request.cwd.as_deref());
        let code = match lease.runtime.workload(&request.command, &cwd) {
            Ok(code) => code,
            Err(reason) => return RunCommandResponse::Denied(denied(reason)),
        };
        let timeout = Duration::from_secs(u64::from(request.timeout_seconds));
        match self.cli.run(lease, request.env.as_map(), &code, timeout).await {
            Err(error) => RunCommandResponse::Denied(denied(error.to_string())),
            Ok(mut output) => {
                let redaction = RedactionSet::from_request_values(&request.redact_values);
                output.stdout = redaction.redact(&output.stdout);
                output.stderr = redaction.redact(&output.stderr);
                RunCommandResponse::Completed(output)
            }
        }
    }

    /// Writes files into `lease`'s workspace from the host side.
    pub async fn write_files(&self, lease: &HlukLease, request: &WriteFilesRequest) -> WriteFilesResponse {
        if request.as_user.is_some() {
            return WriteFilesResponse::Denied(denied("hluk guests have one user; as_user is not supported"));
        }
        if lease.access == WorkspaceAccess::ReadOnly {
            return WriteFilesResponse::Denied(denied("the workspace is a read-only host mount"));
        }
        for file in &request.files {
            let path = match lease.host_path(&file.path) {
                Ok(path) => path,
                Err(reason) => return WriteFilesResponse::Denied(denied(reason)),
            };
            if let Err(error) = write_file(&path, &file.content, file.mode).await {
                return WriteFilesResponse::Denied(denied(format!("writing {}: {error}", file.path)));
            }
        }
        WriteFilesResponse::Written
    }

    /// Reads a file of `lease`'s workspace from the host side.
    pub async fn read_file(&self, lease: &HlukLease, request: &ReadFileRequest) -> ReadFileResponse {
        if request.as_user.is_some() {
            return ReadFileResponse::Denied(denied("hluk guests have one user; as_user is not supported"));
        }
        let path = match lease.host_path(&request.path) {
            Ok(path) => path,
            Err(reason) => return ReadFileResponse::Denied(denied(reason)),
        };
        match tokio::fs::read(&path).await {
            Ok(data) => ReadFileResponse::Content { data },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReadFileResponse::NotFound,
            Err(error) => ReadFileResponse::Denied(denied(format!("reading {}: {error}", request.path))),
        }
    }
}

async fn write_file(path: &std::path::Path, content: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

#[async_trait]
impl Provider for HlukProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Hluk
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        reject_unsupported_privileged(&req.spec, &PROFILE)?;
        reject_unknown_keys(&options_for(ProviderKind::Hluk, req.native.as_ref())?, &[])?;
        let sandbox_id = SandboxId::new(lease_id.as_str()).map_err(ProviderError::backend)?;
        let grant = ProviderLease {
            sandbox_id,
            evidence_subject: evidence_subject(lease_id),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        };
        // A redelivered dispatch keeps the lease it already granted.
        if self.lease_of(lease_id).is_some() {
            return Ok(grant);
        }
        let lease = HlukLease::plan(req, self.work_root.join(lease_id.as_str())).map_err(ProviderError::backend)?;
        self.warm(lease.runtime).await?;
        if lease.owner == WorkspaceOwner::Worker {
            tokio::fs::create_dir_all(&lease.workspace)
                .await
                .map_err(ProviderError::backend)?;
        }
        self.held().insert(lease_id.clone(), Arc::new(lease));
        Ok(grant)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let Some(lease) = self.held().remove(lease_id) else {
            return Ok(());
        };
        if lease.owner == WorkspaceOwner::Worker
            && let Err(error) = tokio::fs::remove_dir_all(&lease.workspace).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(ProviderError::backend(error));
        }
        Ok(())
    }
}
