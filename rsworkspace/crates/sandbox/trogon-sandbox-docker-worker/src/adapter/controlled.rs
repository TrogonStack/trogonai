use super::command_custody::{Absence, NativeIdentity, Owner, Phase, Record, Stored};
use super::{DockerWorkerProvider, ExecStdin, ExecutionState, OutputRetention};
use anyhow::{Context, ensure};
use std::collections::BTreeMap;
use std::time::Duration;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CommandExecution, CommandLaunchOrigin, ExecutionId,
    KillCommandResponse, LaunchKey, LeaseError, ProcessName, ProviderDispatchPayload, RunCommandRequest,
    RunCommandResponse, RunCommandStartedResponse, StdinMode, UnixSignal,
};
use trogon_sandbox_protocol::{VerifiedVerbScope, global_verifier};

#[derive(Clone)]
pub(super) struct ControlledExecution {
    pub owner: Owner,
    pub launch_key: LaunchKey,
    pub controller: super::supervised_exec::Controller,
}

enum NativeCommandSupport {
    Ready(super::command_runtime::RuntimeMount),
    IncompatibleIsolation,
}

fn incompatible_capability(capability: &str) -> bool {
    let canonical = capability.trim().to_ascii_uppercase();
    matches!(
        canonical.strip_prefix("CAP_").unwrap_or(&canonical),
        "ALL" | "SYS_PTRACE" | "SYS_ADMIN"
    )
}

fn incompatible_isolation() -> LeaseError {
    invalid(
        "command launch cancellation requires explicit drop-all capabilities and excludes privileged, ptrace or administration capabilities",
    )
}

struct PreparationGuard(Option<super::supervised_exec::Controller>);
impl Drop for PreparationGuard {
    fn drop(&mut self) {
        if let Some(controller) = &self.0 {
            controller.close();
        }
    }
}

#[expect(
    clippy::result_large_err,
    reason = "LeaseError is the public contract error and is returned as defined"
)]
fn native_identity(ready: &super::supervised_exec::Ready, digest: &str) -> Result<NativeIdentity, LeaseError> {
    Ok(NativeIdentity {
        helper_pid: ready.helper.pid,
        helper_start_time: ready.helper.start_time,
        root_pid: ready.root.pid,
        root_start_time: ready.root.start_time,
        process_group: ready.process_group.try_into().map_err(|_| custody_error())?,
        session: ready.session.try_into().map_err(|_| custody_error())?,
        artifact_digest: digest.into(),
        protocol: sandbox_command_supervisor::protocol::VERSION,
    })
}

fn signal_number(signal: UnixSignal) -> i32 {
    match signal {
        UnixSignal::Hup => 1,
        UnixSignal::Int => 2,
        UnixSignal::Quit => 3,
        UnixSignal::Kill => 9,
        UnixSignal::Usr1 => 10,
        UnixSignal::Usr2 => 12,
        UnixSignal::Term => 15,
    }
}

fn detached_process(request: &RunCommandRequest) -> Option<&trogon_sandbox_contract::ProcessSpec> {
    match &request.execution {
        CommandExecution::Detached { process, .. } => process.as_ref(),
        CommandExecution::Attached => None,
    }
}

fn started_deadline(request: &RunCommandRequest) -> trogon_sandbox_contract::CommandDeadline {
    match &request.execution {
        CommandExecution::Detached { deadline, .. } => *deadline,
        CommandExecution::Attached => trogon_sandbox_contract::CommandDeadline::default(),
    }
}

fn invalid(reason: &str) -> LeaseError {
    LeaseError::InvalidSpec { reason: reason.into() }
}
fn custody_error() -> LeaseError {
    LeaseError::Backend {
        message: "durable command custody could not establish exact ownership".into(),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "LeaseError is the public contract error and is returned as defined"
)]
fn require_fresh_assigned_launch(owner: &Owner) -> Result<(), LeaseError> {
    if owner.target.is_some() {
        global_verifier()
            .fresh_command_launch_fence()
            .map_err(|error| LeaseError::Backend {
                message: error.to_string(),
            })?;
    }
    Ok(())
}

fn pending(message: &str) -> CancelCommandLaunchResponse {
    CancelCommandLaunchResponse::Pending {
        message: message.into(),
    }
}
fn settled(record: &Record) -> CancelCommandLaunchResponse {
    if record.phase == Phase::Preserved {
        match record.execution_id.clone() {
            Some(execution_id) => CancelCommandLaunchResponse::Preserved { execution_id },
            None => pending("preserved command has no execution identity"),
        }
    } else {
        CancelCommandLaunchResponse::Canceled {
            execution_id: record.native.as_ref().and(record.execution_id.clone()),
        }
    }
}

impl DockerWorkerProvider {
    pub async fn bind_command_assignment(&self, payload: &ProviderDispatchPayload) -> anyhow::Result<()> {
        let _gate = self.authority_gate(&payload.lease_id).lock_owned().await;
        let mut active = self.active.lock().await;
        let held = active
            .get_mut(payload.lease_id.as_str())
            .context("no held Docker registration")?;
        ensure!(
            *held.request == payload.request,
            "dispatch changed the admitted lease request"
        );
        if let Some(target) = &payload.target {
            ensure!(
                target.worker_id.as_str() == self.worker_id,
                "dispatch targets a different actual worker"
            );
        }
        if held.dispatch_target_bound {
            ensure!(
                held.dispatch_target == payload.target,
                "dispatch changed its held assignment"
            );
        } else {
            held.dispatch_target = payload.target.clone();
            held.dispatch_target_bound = true;
        }
        Ok(())
    }

    async fn command_owner(
        &self,
        lease: &trogon_sandbox_contract::LeaseId,
        scope: &VerifiedVerbScope,
        subject: &str,
        payload: &[u8],
    ) -> Result<Owner, LeaseError> {
        if !scope.matches(subject, payload) || scope.scope().lease_id != *lease {
            return Err(invalid("command scope does not match the exact payload and lease"));
        }
        let active = self.active.lock().await;
        let held = active.get(lease.as_str()).ok_or_else(custody_error)?;
        if !held.dispatch_target_bound {
            return Err(invalid(
                "command registration has not completed independent dispatch admission",
            ));
        }
        let signed = scope.scope();
        if signed.tenant != held.request.tenant
            || signed
                .worker_id
                .as_ref()
                .is_some_and(|worker| worker.as_str() != self.worker_id)
        {
            return Err(invalid(
                "command scope differs from the admitted tenant or actual worker",
            ));
        }
        match &held.dispatch_target {
            Some(target)
                if signed.worker_id.as_ref().map(|worker| worker.as_str()) == Some(target.worker_id.as_str())
                    && signed.attempt_id.as_ref().map(|attempt| attempt.as_str())
                        == Some(target.attempt_id.as_str())
                    && signed.epoch == target.epoch => {}
            None if signed.attempt_id.is_none() && signed.epoch.is_none() => {}
            _ => {
                return Err(invalid(
                    "command scope differs from the independently admitted assignment",
                ));
            }
        }
        Ok(Owner {
            tenant: held.request.tenant.clone(),
            lease_id: lease.clone(),
            container_id: held.container_id.clone(),
            worker_id: self.worker_id.clone(),
            registration_id: held.registration_id.clone(),
            target: held.dispatch_target.clone(),
        })
    }

    async fn owner_is_held(&self, owner: &Owner) -> bool {
        self.active
            .lock()
            .await
            .get(owner.lease_id.as_str())
            .is_some_and(|held| {
                held.dispatch_target_bound
                    && held.request.tenant == owner.tenant
                    && held.container_id == owner.container_id
                    && held.registration_id == owner.registration_id
                    && held.dispatch_target == owner.target
                    && self.worker_id == owner.worker_id
            })
    }

    pub fn supervises_closed_command(&self, request: &RunCommandRequest) -> bool {
        self.command_runtime.is_some()
            && matches!(
                request.execution,
                CommandExecution::Detached {
                    stdin: StdinMode::Closed,
                    ..
                }
            )
    }

    async fn strong_prerequisites(
        &self,
        owner: &Owner,
        request: &RunCommandRequest,
    ) -> Result<NativeCommandSupport, LeaseError> {
        let runtime = self
            .command_runtime
            .as_ref()
            .ok_or_else(|| invalid("command launch cancellation requires a configured immutable native supervisor"))?;
        let mount = self
            .active
            .lock()
            .await
            .get(owner.lease_id.as_str())
            .and_then(|held| held.runtime_mount.clone())
            .ok_or_else(custody_error)?;
        tokio::time::timeout(Duration::from_secs(15), async {
            runtime.verify_mount(&self.docker, &owner.container_id, &mount).await.map_err(|_| custody_error())?;
            let inspected = self.docker.inspect_container(&owner.container_id, None).await.map_err(|_| custody_error())?;
            if inspected.id.as_deref() != Some(owner.container_id.as_str()) {
                return Err(custody_error());
            }
            let host = inspected.host_config.as_ref().ok_or_else(custody_error)?;
            let drops_all = host.cap_drop.as_ref().is_some_and(|capabilities| capabilities.iter().any(|capability|
                matches!(capability.trim().to_ascii_uppercase().as_str(), "ALL" | "CAP_ALL")));
            if !drops_all || host.privileged == Some(true) || host.cap_add.as_ref().is_some_and(|capabilities|
                capabilities.iter().any(|capability| incompatible_capability(capability))) {
                return Ok(NativeCommandSupport::IncompatibleIsolation);
            }
            let output = Self::exec_collect_capped(
                &self.docker, &owner.container_id, vec![mount.executable(), "--probe".into()],
                None, &BTreeMap::new(), ExecStdin::None,
                request.run_as_user.as_ref().map(|user| user.as_str()),
                None, None, OutputRetention::Buffer, Some(1024), None,
                &trogon_sandbox_protocol::RedactionSet::default(),
            ).await.map_err(|_| custody_error())?;
            if output.exit_code != 0 || !output.stderr.is_empty() || output.truncated.is_truncated() {
                return Err(custody_error());
            }
            let probed = sandbox_command_supervisor::protocol::decode_event(output.stdout.trim_ascii())
                .map(sandbox_command_supervisor::protocol::Event::into_wire);
            if !matches!(probed,
                Ok(sandbox_command_supervisor::protocol::EventWire::Probe { version, supported: sandbox_command_supervisor::protocol::PlatformSupport::Supported })
                if version == sandbox_command_supervisor::protocol::VERSION) {
                return Err(custody_error());
            }
            Ok(NativeCommandSupport::Ready(mount))
        }).await.map_err(|_| custody_error())?
    }

    pub async fn run_controlled_command(
        &self,
        request: &RunCommandRequest,
        scope: &VerifiedVerbScope,
        subject: &str,
        payload: &[u8],
    ) -> RunCommandResponse {
        let result = async {
            let parsed: RunCommandRequest =
                serde_json::from_slice(payload).map_err(|_| invalid("invalid command payload"))?;
            if parsed != *request {
                return Err(invalid("command payload changed after verification"));
            }
            let owner = self.command_owner(&request.lease_id, scope, subject, payload).await?;
            if request.command.is_empty() {
                return Err(invalid("strong command has an empty argv"));
            }
            let key = if request.require_launch_cancellation.is_required() {
                request
                    .launch_key
                    .clone()
                    .ok_or_else(|| invalid("strong cancellation requires a launch key"))?
            } else {
                LaunchKey::new(format!("command-{}", crate::fresh_id::fresh_id()))
                    .map_err(|_| invalid("generated launch key is invalid"))?
            };
            let CommandExecution::Detached {
                stdin: StdinMode::Closed,
                deadline,
                process,
            } = &request.execution
            else {
                return Err(invalid(
                    "strong cancellation requires detached execution with closed stdin",
                ));
            };
            let _gate = self.authority_gate(&request.lease_id).lock_owned().await;
            if !self.owner_is_held(&owner).await {
                return Err(custody_error());
            }
            match self.strong_prerequisites(&owner, request).await? {
                NativeCommandSupport::Ready(mount) => self.start_controlled(request, owner, &key, mount).await,
                NativeCommandSupport::IncompatibleIsolation if !request.require_launch_cancellation.is_required() => {
                    self.start_detached(
                        &request.lease_id,
                        request.command.clone(),
                        request.cwd.clone(),
                        request.env.as_map(),
                        request.timeout_seconds,
                        request.run_as_user.as_ref(),
                        request.stream_output.is_enabled(),
                        StdinMode::Closed,
                        request.output_retention,
                        *deadline,
                        trogon_sandbox_protocol::effective_max_output_bytes(request.max_output_bytes, usize::MAX),
                        process.clone(),
                        trogon_sandbox_protocol::RedactionSet::from_request_values(&request.redact_values),
                        None,
                        None,
                    )
                    .await
                    .map_err(|_| custody_error())
                }
                NativeCommandSupport::IncompatibleIsolation => Err(incompatible_isolation()),
            }
        }
        .await;
        match result {
            Ok(started) => RunCommandResponse::Started(started),
            Err(error) => RunCommandResponse::Denied(error),
        }
    }

    async fn start_controlled(
        &self,
        request: &RunCommandRequest,
        owner: Owner,
        key: &LaunchKey,
        mount: super::command_runtime::RuntimeMount,
    ) -> Result<RunCommandStartedResponse, LeaseError> {
        let fingerprint = super::launch_fingerprint::launch_fingerprint(request).map_err(|_| custody_error())?;
        let mut reserved = None;
        for _ in 0..16 {
            if let Some(stored) = self
                .command_custody
                .load(&owner, key)
                .await
                .map_err(|_| custody_error())?
            {
                if stored.record.phase == Phase::Canceled || stored.record.cancel_requested {
                    return Err(invalid("the command launch was canceled"));
                }
                if stored.record.fingerprint.as_deref() != Some(fingerprint.as_str()) {
                    return Err(invalid("launch key was retained for another command"));
                }
                if matches!(stored.record.phase, Phase::Running | Phase::Preserved) {
                    let execution_id = stored.record.execution_id.ok_or_else(custody_error)?;
                    if !self.executions.lock().await.contains_key(&execution_id) {
                        return Err(custody_error());
                    }
                    if stored.record.phase == Phase::Running {
                        let controls = self.controlled_executions.lock().await;
                        let control = controls.get(&execution_id).ok_or_else(custody_error)?;
                        if control.owner != owner || control.launch_key != *key || !control.controller.released() {
                            return Err(custody_error());
                        }
                    }
                    return Ok(RunCommandStartedResponse {
                        execution_id,
                        origin: stored.record.origin.ok_or_else(custody_error)?,
                    });
                }
                return Err(custody_error());
            }
            require_fresh_assigned_launch(&owner)?;
            let reused = if let Some(name) =
                detached_process(request).and_then(|process| process.name.as_ref().map(ProcessName::as_str))
            {
                let processes = self.processes.lock().await;
                let executions = self.executions.lock().await;
                processes
                    .iter()
                    .find(|(id, meta)| {
                        meta.lease_id == request.lease_id
                            && meta.name.as_ref().map(ProcessName::as_str) == Some(name)
                            && matches!(executions.get(*id), Some(ExecutionState::Running { .. }))
                    })
                    .map(|(id, _)| id.clone())
            } else {
                None
            };
            let execution_id = reused
                .clone()
                .unwrap_or_else(|| ExecutionId::new(crate::fresh_id::fresh_id()));
            if let Some(stored) = self
                .command_custody
                .create(Record::reserved(
                    owner.clone(),
                    key,
                    fingerprint.clone(),
                    execution_id.clone(),
                    reused.is_some(),
                ))
                .await
                .map_err(|_| custody_error())?
            {
                if reused.is_some() {
                    return Ok(RunCommandStartedResponse {
                        execution_id,
                        origin: CommandLaunchOrigin::Reused,
                    });
                }
                reserved = Some(stored);
                break;
            }
        }
        let reserved = reserved.ok_or_else(custody_error)?;
        let mut preparing = reserved.record.clone();
        preparing.phase = Phase::Preparing;
        let preparing = self
            .command_custody
            .replace(&reserved, preparing)
            .await
            .map_err(|_| custody_error())?
            .ok_or_else(custody_error)?;
        let execution_id = preparing.record.execution_id.clone().ok_or_else(custody_error)?;
        let (controller, launch) =
            super::supervised_exec::prepare(&execution_id, request.clone()).map_err(|_| custody_error())?;
        let mut guard = PreparationGuard(Some(controller.clone()));
        require_fresh_assigned_launch(&owner)?;
        let started = self
            .start_detached(
                &request.lease_id,
                vec![
                    mount.executable(),
                    "--execution-id".into(),
                    execution_id.as_str().to_owned(),
                ],
                request.cwd.clone(),
                request.env.as_map(),
                request.timeout_seconds,
                request.run_as_user.as_ref(),
                request.stream_output.is_enabled(),
                StdinMode::Closed,
                request.output_retention,
                started_deadline(request),
                trogon_sandbox_protocol::effective_max_output_bytes(request.max_output_bytes, usize::MAX),
                detached_process(request).cloned(),
                trogon_sandbox_protocol::RedactionSet::from_request_values(&request.redact_values),
                Some(execution_id.clone()),
                Some(launch),
            )
            .await
            .map_err(|_| custody_error())?;
        if started.execution_id != execution_id || started.origin != CommandLaunchOrigin::Created {
            return Err(custody_error());
        }
        if let Some(launch) = self.launches.lock().await.get_mut(&execution_id) {
            launch.command = request.command.clone();
            launch.controlled_request = Some(Box::new(request.clone()));
        } else {
            return Err(custody_error());
        }
        let ready = controller.ready().await.map_err(|_| custody_error())?;
        let native = native_identity(&ready, &mount.digest)?;
        let mut bound = None;
        for _ in 0..16 {
            let current = self
                .command_custody
                .load(&owner, key)
                .await
                .map_err(|_| custody_error())?
                .ok_or_else(custody_error)?;
            if current.record.phase != Phase::Preparing {
                return Err(custody_error());
            }
            let mut next = current.record.clone();
            next.native = Some(native.clone());
            next.phase = Phase::Bound;
            if let Some(stored) = self
                .command_custody
                .replace(&current, next)
                .await
                .map_err(|_| custody_error())?
            {
                bound = Some(stored);
                break;
            }
        }
        let bound = bound.ok_or_else(custody_error)?;
        self.controlled_executions.lock().await.insert(
            execution_id.clone(),
            ControlledExecution {
                owner: owner.clone(),
                launch_key: key.clone(),
                controller: controller.clone(),
            },
        );
        if bound.record.cancel_requested {
            let _ = self.cancel_controlled_record(bound).await;
            return Err(invalid("the command launch was canceled"));
        }
        let mut running = bound.record.clone();
        running.phase = Phase::Running;
        if self
            .command_custody
            .replace(&bound, running)
            .await
            .map_err(|_| custody_error())?
            .is_none()
        {
            return Err(custody_error());
        }
        require_fresh_assigned_launch(&owner)?;
        controller.release().await.map_err(|_| custody_error())?;
        guard.0 = None;
        Ok(started)
    }

    pub async fn cancel_controlled_command(
        &self,
        request: &CancelCommandLaunchRequest,
        scope: &VerifiedVerbScope,
        subject: &str,
        payload: &[u8],
    ) -> CancelCommandLaunchResponse {
        if !serde_json::from_slice::<CancelCommandLaunchRequest>(payload).is_ok_and(|parsed| parsed == *request) {
            return CancelCommandLaunchResponse::Denied(invalid("command cancellation payload changed"));
        }
        let owner = match self.command_owner(&request.lease_id, scope, subject, payload).await {
            Ok(owner) => owner,
            Err(error) => return CancelCommandLaunchResponse::Denied(error),
        };
        self.cancel_by_owner(owner, &request.launch_key).await
    }

    async fn cancel_by_owner(&self, owner: Owner, key: &LaunchKey) -> CancelCommandLaunchResponse {
        for _ in 0..16 {
            let stored = match self.command_custody.load(&owner, key).await {
                Ok(Some(stored)) => stored,
                Ok(None) => match self
                    .command_custody
                    .create(Record::canceled_before_start(owner.clone(), key))
                    .await
                {
                    Ok(Some(stored)) => {
                        let _gate = self.authority_gate(&owner.lease_id).lock_owned().await;
                        if !self.owner_is_held(&owner).await {
                            return pending("command registration changed before cancellation settled");
                        }
                        return settled(&stored.record);
                    }
                    Ok(None) => continue,
                    Err(_) => return pending("durable cancellation intent is unavailable"),
                },
                Err(_) => return pending("command custody or its exact owner is unavailable"),
            };
            if matches!(stored.record.phase, Phase::Canceled | Phase::Preserved) {
                let _gate = self.authority_gate(&owner.lease_id).lock_owned().await;
                if !self.owner_is_held(&owner).await {
                    return pending("command registration changed before cancellation replay");
                }
                return settled(&stored.record);
            }
            let stored = if stored.record.cancel_requested {
                stored
            } else {
                let mut next = stored.record.clone();
                next.cancel_requested = true;
                if next.phase == Phase::Reserved {
                    next.phase = Phase::Canceled;
                }
                match self.command_custody.replace(&stored, next).await {
                    Ok(Some(stored)) => stored,
                    Ok(None) => continue,
                    Err(_) => return pending("durable cancellation intent is unavailable"),
                }
            };
            if stored.record.phase == Phase::Canceled {
                let _gate = self.authority_gate(&owner.lease_id).lock_owned().await;
                if !self.owner_is_held(&owner).await {
                    return pending("command registration changed before cancellation settled");
                }
                return settled(&stored.record);
            }
            if stored.record.native.is_none() {
                return pending("original bootstrap has not established exact native custody");
            }
            let _gate = self.authority_gate(&owner.lease_id).lock_owned().await;
            return self.cancel_controlled_record(stored).await;
        }
        pending("command custody changed concurrently")
    }

    async fn cancel_controlled_record(&self, mut stored: Stored) -> CancelCommandLaunchResponse {
        for _ in 0..16 {
            if !self.owner_is_held(&stored.record.owner).await {
                return pending("command registration or native container changed");
            }
            if matches!(stored.record.phase, Phase::Canceled | Phase::Preserved) {
                return settled(&stored.record);
            }
            if !stored.record.cancel_requested {
                return pending("durable cancellation intent is not established");
            }
            let Some(native) = &stored.record.native else {
                return pending("native bootstrap ownership remains pending");
            };
            let Some(execution_id) = &stored.record.execution_id else {
                return pending("native execution identity is unavailable");
            };
            let controlled = match self.controlled_executions.lock().await.get(execution_id).cloned() {
                Some(controlled)
                    if controlled.owner == stored.record.owner && controlled.launch_key == stored.record.launch_key =>
                {
                    controlled
                }
                _ => return pending("exact local supervisor control is unavailable"),
            };
            let ready = match controlled.controller.ready().await {
                Ok(ready) => ready,
                Err(_) => return pending("native supervisor identity is unavailable"),
            };
            if native_identity(&ready, &native.artifact_digest).as_ref().ok() != Some(native) {
                return pending("native supervisor binding changed");
            }
            if controlled.controller.signal(9).await.is_err() {
                return pending("native hard cancellation acknowledgement is unconfirmed");
            }
            let absence = match controlled.controller.absent().await {
                Ok(absence) => absence,
                Err(_) => return pending("complete native family absence remains unconfirmed"),
            };
            if !self.wait_completed(execution_id, Duration::from_secs(5)).await {
                return pending("native output completion remains unconfirmed");
            }
            if !matches!(self.executions.lock().await.get(execution_id),
                Some(ExecutionState::Completed { lease_id, .. }) if *lease_id == stored.record.owner.lease_id)
            {
                return pending("exact native output completion is unavailable");
            }
            let mut next = stored.record.clone();
            next.phase = Phase::Canceled;
            next.absence = Some(Absence {
                released: absence.released,
                root_exit_code: absence.root_exit_code,
                root_signal: absence.root_signal,
                timed_out: absence.timed_out,
                canceled: absence.canceled,
                reaped_count: absence.reaped_count,
            });
            match self.command_custody.replace(&stored, next).await {
                Ok(Some(stored)) => return settled(&stored.record),
                Ok(None) => {}
                Err(_) => return pending("native cleanup could not be durably settled"),
            }
            stored = match self
                .command_custody
                .load(&stored.record.owner, &stored.record.launch_key)
                .await
            {
                Ok(Some(stored)) => stored,
                _ => return pending("command custody became unavailable"),
            };
        }
        pending("command custody changed concurrently")
    }

    pub(super) async fn kill_controlled(
        &self,
        controlled: &ControlledExecution,
        signal: UnixSignal,
    ) -> KillCommandResponse {
        let owner = &controlled.owner;
        if !self.owner_is_held(owner).await {
            return KillCommandResponse::Denied(custody_error());
        }
        let stored = match self.command_custody.load(owner, &controlled.launch_key).await {
            Ok(Some(stored)) => stored,
            _ => return KillCommandResponse::Denied(custody_error()),
        };
        let Some(execution_id) = stored.record.execution_id.as_ref() else {
            return KillCommandResponse::NotFound;
        };
        if matches!(self.executions.lock().await.get(execution_id), Some(ExecutionState::Completed { lease_id, .. }) if *lease_id == owner.lease_id)
        {
            return KillCommandResponse::AlreadyCompleted;
        }
        if signal != UnixSignal::Kill {
            let _gate = self.authority_gate(&owner.lease_id).lock_owned().await;
            if !self.owner_is_held(owner).await {
                return KillCommandResponse::Denied(custody_error());
            }
            let ready = match controlled.controller.ready().await {
                Ok(ready) => ready,
                Err(_) => return KillCommandResponse::Denied(custody_error()),
            };
            let Some(native) = &stored.record.native else {
                return KillCommandResponse::Denied(custody_error());
            };
            if native_identity(&ready, &native.artifact_digest).as_ref().ok() != Some(native) {
                return KillCommandResponse::Denied(custody_error());
            }
            return match controlled.controller.signal(signal_number(signal)).await {
                Ok(true) => KillCommandResponse::Signaled,
                Ok(false) if controlled.controller.completion().is_some() => KillCommandResponse::AlreadyCompleted,
                _ => KillCommandResponse::Denied(custody_error()),
            };
        }
        match self.cancel_by_owner(owner.clone(), &controlled.launch_key).await {
            CancelCommandLaunchResponse::Canceled { .. } => KillCommandResponse::Signaled,
            _ => KillCommandResponse::Denied(custody_error()),
        }
    }

    pub(super) async fn restart_controlled(
        &self,
        lease_id: &trogon_sandbox_contract::LeaseId,
        previous_id: &ExecutionId,
        mut request: RunCommandRequest,
    ) -> Result<RunCommandStartedResponse, LeaseError> {
        let controlled = self
            .controlled_executions
            .lock()
            .await
            .get(previous_id)
            .cloned()
            .ok_or_else(custody_error)?;
        let owner = controlled.owner;
        let _gate = self.authority_gate(lease_id).lock_owned().await;
        if owner.lease_id != *lease_id || !self.owner_is_held(&owner).await {
            return Err(custody_error());
        }
        let mount = match self.strong_prerequisites(&owner, &request).await? {
            NativeCommandSupport::Ready(mount) => mount,
            NativeCommandSupport::IncompatibleIsolation => return Err(incompatible_isolation()),
        };
        let key = LaunchKey::new(format!("restart-{}", crate::fresh_id::fresh_id()))
            .map_err(|_| invalid("generated launch key is invalid"))?;
        request.launch_key = Some(key.clone());
        self.start_controlled(&request, owner, &key, mount).await
    }
}
