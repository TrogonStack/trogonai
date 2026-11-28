use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::OnceCell;
use trogon_sandbox_contract::{
    CommandExecution, LaunchCancellation, LeaseError, LeaseId, LeaseRequest, OutputTruncation, ProviderKind,
    ProviderLease, RunCommandRequest, RunCommandResponse, SandboxId,
};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{
    Provider, ProviderError, RedactionSet, cap_buffered, effective_max_output_bytes, options_for, reject_unknown_keys,
    reject_unsupported_privileged, resolve_exec_cwd,
};

use crate::constants::{ONE_USER, WORKSPACE_GUEST_DIR};
use crate::{HlukCli, HlukLease, PROFILE};

fn denied(message: impl Into<String>) -> RunCommandResponse {
    RunCommandResponse::Denied(LeaseError::Backend {
        message: message.into(),
    })
}

/// Serves leases with `hluk`: a lease is a runtime whose warm snapshot exists plus host directories for its
/// `/workspace` and `/tmp`, and each
/// command is one `hluk run` restored from that snapshot. Nothing in the guest outlives a command except the
/// workspace and `/tmp`.
pub struct HlukProvider {
    cli: HlukCli,
    work_root: PathBuf,
    leases: Mutex<HashMap<LeaseId, Arc<HlukLease>>>,
    /// Set once each runtime's snapshot is saved, indexed by `HlukRuntime`; runtimes warm independently.
    warmed: [OnceCell<()>; 3],
}

impl HlukProvider {
    /// A provider running `cli`, keeping each lease's directories under `work_root`.
    pub fn new(cli: HlukCli, work_root: PathBuf) -> Self {
        Self {
            cli,
            work_root,
            leases: Mutex::default(),
            warmed: Default::default(),
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

    /// Runs `request` on `lease` to completion in a guest restored for it.
    pub async fn run_command(&self, lease: &HlukLease, request: RunCommandRequest) -> RunCommandResponse {
        if matches!(request.execution, CommandExecution::Detached { .. }) {
            return denied(
                "hluk restores a fresh guest per command and keeps none running: detached commands are not supported",
            );
        }
        if request.run_as_user.is_some() {
            return denied(ONE_USER);
        }
        if request.require_launch_cancellation == LaunchCancellation::Required {
            return denied("command launch cancellation is not supported by hluk");
        }
        let cwd = resolve_exec_cwd(WORKSPACE_GUEST_DIR, request.cwd.as_deref());
        let code = match lease.runtime.workload(&request.command, &cwd) {
            Ok(code) => code,
            Err(reason) => return denied(reason),
        };
        let timeout = Duration::from_secs(u64::from(request.timeout_seconds));
        let mut output = match self.cli.run(lease, request.env.as_map(), &code, timeout).await {
            Ok(output) => output,
            Err(error) => return denied(error.to_string()),
        };
        if !request.redact_values.is_empty() {
            let redaction = RedactionSet::from_request_values(&request.redact_values);
            output.stdout = redaction.redact(&output.stdout);
            output.stderr = redaction.redact(&output.stderr);
        }
        let cap = effective_max_output_bytes(request.max_output_bytes, usize::MAX);
        let (stdout, stdout_cut) = cap_buffered(std::mem::take(&mut output.stdout), cap);
        let (stderr, stderr_cut) = cap_buffered(std::mem::take(&mut output.stderr), cap);
        output.stdout = stdout;
        output.stderr = stderr;
        if stderr_cut == OutputTruncation::Truncated {
            output.truncated = stderr_cut;
        } else {
            output.truncated = stdout_cut;
        }
        RunCommandResponse::Completed(output)
    }
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
        self.warmed[lease.runtime as usize]
            .get_or_try_init(|| self.cli.warm(lease.runtime))
            .await
            .map_err(ProviderError::backend)?;
        lease.create_dirs().await.map_err(ProviderError::backend)?;
        self.held().insert(lease_id.clone(), Arc::new(lease));
        Ok(grant)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let Some(lease) = self.held().remove(lease_id) else {
            return Ok(());
        };
        // A caller's read-only workspace lives elsewhere, so this never removes it.
        if let Err(error) = tokio::fs::remove_dir_all(&lease.dir).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(ProviderError::backend(error));
        }
        Ok(())
    }
}
