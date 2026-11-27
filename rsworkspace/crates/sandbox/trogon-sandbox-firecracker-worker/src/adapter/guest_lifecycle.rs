use super::*;
use trogon_sandbox_contract::{
    CommandEnv, CommandExecution, LaunchCancellation, RedactValues, SandboxId, StreamedOutput,
};

/// The base rootfs is shared by every lease and cannot know each caller's working
/// directory, and the agent refuses to spawn into one that is missing.
pub(super) async fn ensure_working_dir(agent: &GuestAgent, lease_id: &LeaseId, dir: &str) -> Result<(), ProviderError> {
    let request = RunCommandRequest {
        require_launch_cancellation: LaunchCancellation::NotRequired,
        launch_key: None,
        lease_id: lease_id.clone(),
        command: vec!["mkdir".to_string(), "-p".to_string(), dir.to_string()],
        cwd: Some("/".to_string()),
        env: CommandEnv::default(),
        timeout_seconds: 30,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: Default::default(),
        max_output_bytes: None,
        redact_values: RedactValues::default(),
        execution: CommandExecution::Attached,
    };
    let failed = |reason: String| ProviderError::WorkingDir {
        dir: dir.to_string(),
        reason,
    };
    match agent.exec(&request, &DiscardChunks).await? {
        RunCommandResponse::Completed(output) if output.exit_code == 0 => Ok(()),
        RunCommandResponse::Completed(output) => Err(failed(format!(
            "mkdir exited {}: {}",
            output.exit_code,
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
        other => Err(failed(format!("unexpected exec response: {other:?}"))),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn cleanup_lost(
    active: Arc<Mutex<HashMap<String, ActiveVm>>>,
    losses: Arc<Mutex<HashMap<String, LeaseLoss>>>,
    launcher: Arc<dyn VmLauncher>,
    gates: Arc<crate::retained::LeaseGates>,
    sink: Option<Arc<dyn LeaseLossSink>>,
    lifecycle: Option<EmitterFactory>,
    resources: Arc<crate::attempt_resources::AttemptResources>,
    lease: LeaseId,
    generation: u64,
    status: String,
) {
    let _gate = gates.lock(&lease).await;
    let loss = LeaseLoss::VmExited { status };
    let mut vm = {
        let mut active = active.lock().await;
        if active.get(lease.as_str()).is_none_or(|vm| vm.generation != generation) {
            return;
        }
        losses.lock().await.insert(lease.as_str().to_owned(), loss.clone());
        match active.remove(lease.as_str()) {
            Some(vm) => vm,
            None => return,
        }
    };
    vm.process.stop().await;
    let cleaned = async {
        if let Some(network) = vm.network.as_mut() {
            network.shutdown(&vm.dir).await?;
        }
        launcher.cleanup(&vm.dir).await?;
        vm.dir.remove().await?;
        if let Some(resource) = &vm.resource {
            resources.complete(resource).await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = cleaned {
        warn!(lease_id=%lease.as_str(),%error,"lost VM cleanup remains pending");
        active.lock().await.insert(lease.as_str().to_owned(), vm);
    }
    if let Some(factory) = lifecycle
        && let Ok(sandbox_id) = SandboxId::new(lease.as_str())
    {
        factory
            .build(&lease)
            .emit(trogon_sandbox_contract::LifecycleEvent::Failed {
                sandbox_id,
                error: loss.to_string(),
            })
            .await;
    }
    if let Some(sink) = sink {
        sink.lost(&lease, &loss).await;
    }
}

pub(super) fn lease_of(sandbox_id: &str, lease_id: &LeaseId) -> anyhow::Result<ProviderLease> {
    Ok(ProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new(sandbox_id)?,
        evidence_subject: trogon_sandbox_protocol::subjects::evidence_subject(lease_id),
        worker_id: None,
        attempt_id: None,
    })
}

pub(super) async fn refresh_guest_identity(agent: &GuestAgent, lease_id: &LeaseId) -> Result<(), ProviderError> {
    agent.verify_restore_clock_source(lease_id).await?;
    let response = agent
        .exec(
            &RunCommandRequest {
                require_launch_cancellation: LaunchCancellation::NotRequired,
                launch_key: None,
                lease_id: lease_id.clone(),
                command: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "mkdir -p /run/ate && printf '%s' \"$1\" > /run/ate/actor-id".into(),
                    "sh".into(),
                    lease_id.as_str().to_owned(),
                ],
                cwd: Some("/".into()),
                env: CommandEnv::default(),
                timeout_seconds: 5,
                stream_output: StreamedOutput::Disabled,
                run_as_user: None,
                output_retention: Default::default(),
                max_output_bytes: None,
                redact_values: RedactValues::default(),
                execution: CommandExecution::Attached,
            },
            &DiscardChunks,
        )
        .await?;
    match response {
        RunCommandResponse::Completed(output) if output.exit_code == 0 => Ok(()),
        other => Err(ProviderError::Launch(format!(
            "refreshing restored guest identity failed: {other:?}"
        ))),
    }
}

/// Recovers the typed error `run_phase` boxed into `anyhow`.
pub(super) fn downcast(error: anyhow::Error) -> ProviderError {
    match error.downcast::<ProviderError>() {
        Ok(typed) => typed,
        Err(other) => ProviderError::Launch(other.to_string()),
    }
}
