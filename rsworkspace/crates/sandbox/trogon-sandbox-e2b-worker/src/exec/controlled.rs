use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, ExecutionId, LeaseError, ProcessState, ProviderKind,
    RunCommandRequest, SandboxUserName,
};
use trogon_sandbox_protocol::{VerifiedVerbScope, subjects};

use super::command_owner::CommandOwner;
use super::command_phase::CommandPhase;
use super::command_record::CommandRecord;
use super::constants::{KILL_SCRIPT, MAX_CUSTODY_ATTEMPTS, RELEASE_SCRIPT};
use super::deadline::Deadline;
use super::e2b_exec::E2bExec;
use super::execution::Execution;
use super::failure::{custody_unavailable, invalid_spec};
use super::launch_fingerprint::LaunchFingerprint;
use super::launch_release::LaunchRelease;
use super::stored_command::StoredCommand;
use crate::LeaseSandbox;
use crate::snapshots::CapturedExecution;

fn pending(message: &str) -> CancelCommandLaunchResponse {
    CancelCommandLaunchResponse::Pending {
        message: message.into(),
    }
}

fn canceled(record: &CommandRecord) -> CancelCommandLaunchResponse {
    CancelCommandLaunchResponse::Canceled {
        execution_id: record
            .execution
            .as_ref()
            .map(|execution| ExecutionId::new(execution.execution_id.clone())),
    }
}

/// The record with cancellation requested, and settled at once when nothing was launched yet.
fn cancellation_requested(record: &CommandRecord) -> CommandRecord {
    let mut next = record.clone();
    next.cancel_requested = true;
    if next.phase == CommandPhase::Reserved {
        next.phase = CommandPhase::Canceled;
    }
    next
}

impl E2bExec {
    pub(super) async fn start_controlled(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        owner: CommandOwner,
    ) -> Result<ExecutionId, LeaseError> {
        let _gate = self.freeze_executions(request.lease_id.as_str()).await;
        let key = request
            .launch_key
            .as_ref()
            .ok_or_else(|| invalid_spec("launch cancellation requires a launch_key"))?;
        let fingerprint =
            LaunchFingerprint::of(request).map_err(|_| invalid_spec("the command launch cannot be fingerprinted"))?;
        let mut stored = None;
        for _ in 0..MAX_CUSTODY_ATTEMPTS {
            let candidate = match self.command_custody.load(&owner, key).await? {
                Some(stored) => stored,
                None => {
                    let reserved = CommandRecord::reserved(owner.clone(), key.clone(), fingerprint.clone());
                    let Some(stored) = self.command_custody.create(reserved).await? else {
                        continue;
                    };
                    stored
                }
            };
            if candidate.record.phase == CommandPhase::Canceled || candidate.record.cancel_requested {
                return Err(invalid_spec("the command launch was canceled"));
            }
            if candidate.record.fingerprint.as_ref() != Some(&fingerprint) {
                return Err(invalid_spec("launch_key was already used for a different command"));
            }
            if candidate.record.phase == CommandPhase::Running {
                let execution = candidate.record.execution.as_ref().ok_or_else(custody_unavailable)?;
                self.restore_controlled_execution(sandbox, &owner, execution).await?;
                return Ok(ExecutionId::new(execution.execution_id.clone()));
            }
            if candidate.record.phase != CommandPhase::Reserved {
                return Err(LeaseError::Backend {
                    message: "command launch outcome is pending its original native bootstrap".into(),
                });
            }
            self.check_fresh_worker_launch()?;
            let mut preparing = candidate.record.clone();
            preparing.phase = CommandPhase::Preparing;
            if let Some(owned) = self.command_custody.replace(&candidate, preparing).await? {
                stored = Some(owned);
                break;
            }
        }
        let stored = stored.ok_or_else(custody_unavailable)?;
        let execution_id = stored.record.execution_id.clone().ok_or_else(custody_unavailable)?;
        self.check_fresh_worker_launch()?;
        let execution = self
            .prepare_execution(sandbox, request, &execution_id, LaunchRelease::Deferred)
            .await?;
        let captured = CapturedExecution {
            execution_id: execution_id.to_string(),
            user: SandboxUserName::new(execution.user.clone()).map_err(|_| custody_unavailable())?,
            identity: execution.identity,
            max_output_bytes: execution.max_output_bytes as u64,
            deadline: Some(execution.deadline.capture().await?),
        };
        let bound = self
            .bind_launch(&owner, request, &execution_id, &fingerprint, &captured)
            .await?;
        if bound.record.cancel_requested {
            let _ = self.cancel_controlled_record(sandbox, bound).await;
            return Err(invalid_spec("the command launch was canceled"));
        }
        let mut running = bound.record.clone();
        running.phase = CommandPhase::Running;
        if self.command_custody.replace(&bound, running).await?.is_none() {
            return Err(LeaseError::Backend {
                message: "command launch ownership changed before native release".into(),
            });
        }
        self.check_fresh_worker_launch()?;
        self.run_script(
            sandbox,
            &execution.user,
            RELEASE_SCRIPT,
            &[
                execution.dir.clone(),
                execution.identity.pid.to_string(),
                execution.identity.started.to_string(),
            ],
            &BTreeMap::new(),
            None,
        )
        .await?;
        self.remember_execution(sandbox, &request.lease_id, &execution_id, execution)
            .await;
        Ok(execution_id)
    }

    /// Records the native process a preparing launch started, so cancellation can find exactly it.
    async fn bind_launch(
        &self,
        owner: &CommandOwner,
        request: &RunCommandRequest,
        execution_id: &ExecutionId,
        fingerprint: &LaunchFingerprint,
        captured: &CapturedExecution,
    ) -> Result<StoredCommand, LeaseError> {
        let key = request.launch_key.as_ref().ok_or_else(custody_unavailable)?;
        for _ in 0..MAX_CUSTODY_ATTEMPTS {
            let current = self
                .command_custody
                .load(owner, key)
                .await?
                .ok_or_else(custody_unavailable)?;
            if current.record.phase != CommandPhase::Preparing
                || current.record.execution_id.as_ref() != Some(execution_id)
                || current.record.fingerprint.as_ref() != Some(fingerprint)
            {
                return Err(custody_unavailable());
            }
            let mut next = current.record.clone();
            next.phase = CommandPhase::Bound;
            next.execution = Some(captured.clone());
            if let Some(stored) = self.command_custody.replace(&current, next).await? {
                return Ok(stored);
            }
        }
        Err(custody_unavailable())
    }

    async fn restore_controlled_execution(
        &self,
        sandbox: &LeaseSandbox,
        owner: &CommandOwner,
        captured: &CapturedExecution,
    ) -> Result<(), LeaseError> {
        let execution_id = ExecutionId::new(captured.execution_id.clone());
        if self.execution(&owner.lease_id, &execution_id).await.is_some() {
            return Ok(());
        }
        let mut execution = Self::controlled_execution(owner, captured);
        let state = self.process_state(sandbox, &execution).await?;
        execution.deadline = Deadline::restore(captured.deadline.as_ref(), state)?;
        self.remember_execution(sandbox, &owner.lease_id, &execution_id, execution)
            .await;
        Ok(())
    }

    fn controlled_execution(owner: &CommandOwner, captured: &CapturedExecution) -> Execution {
        Execution {
            lease_id: owner.lease_id.clone(),
            dir: captured.directory(),
            user: captured.user.as_str().into(),
            identity: captured.identity,
            max_output_bytes: usize::try_from(captured.max_output_bytes).unwrap_or(usize::MAX),
            deadline: Deadline::Lease,
        }
    }

    pub(super) async fn cancel_controlled(
        &self,
        sandbox: &LeaseSandbox,
        request: &CancelCommandLaunchRequest,
        owner: CommandOwner,
    ) -> CancelCommandLaunchResponse {
        for _ in 0..MAX_CUSTODY_ATTEMPTS {
            let stored = match self.command_custody.load(&owner, &request.launch_key).await {
                Ok(Some(stored)) => stored,
                Ok(None) => {
                    let record = CommandRecord::canceled_before_start(owner.clone(), request.launch_key.clone());
                    match self.command_custody.create(record).await {
                        Ok(Some(stored)) => return canceled(&stored.record),
                        Ok(None) => continue,
                        Err(_) => return pending("durable cancellation intent could not be established"),
                    }
                }
                Err(_) => return pending("command ownership or durable custody could not be established"),
            };
            let stored = if stored.record.cancel_requested {
                stored
            } else {
                match self
                    .command_custody
                    .replace(&stored, cancellation_requested(&stored.record))
                    .await
                {
                    Ok(Some(stored)) => stored,
                    Ok(None) => continue,
                    Err(_) => return pending("durable cancellation intent could not be established"),
                }
            };
            if stored.record.phase == CommandPhase::Canceled {
                return canceled(&stored.record);
            }
            if stored.record.execution.is_none() {
                return pending("the original native bootstrap has not established exact process custody");
            }
            let _gate = self.freeze_executions(request.lease_id.as_str()).await;
            return self.cancel_controlled_record(sandbox, stored).await;
        }
        pending("command custody is changing concurrently")
    }

    async fn cancel_controlled_record(
        &self,
        sandbox: &LeaseSandbox,
        mut stored: StoredCommand,
    ) -> CancelCommandLaunchResponse {
        for _ in 0..MAX_CUSTODY_ATTEMPTS {
            if stored.record.phase == CommandPhase::Canceled {
                return canceled(&stored.record);
            }
            if let Some(captured) = stored.record.execution.as_ref() {
                let execution = Self::controlled_execution(&stored.record.owner, captured);
                let Ok(state) = self.process_state(sandbox, &execution).await else {
                    return pending("exact native command identity could not be confirmed");
                };
                if state == ProcessState::Running {
                    let stopped = self
                        .run_script(
                            sandbox,
                            &execution.user,
                            KILL_SCRIPT,
                            &[
                                execution.dir.clone(),
                                "KILL".into(),
                                execution.identity.pid.to_string(),
                                execution.identity.started.to_string(),
                            ],
                            &BTreeMap::new(),
                            None,
                        )
                        .await;
                    if stopped.is_err() {
                        return pending("exact native cancellation remains unconfirmed");
                    }
                }
                if !matches!(
                    self.process_state(sandbox, &execution).await,
                    Ok(ProcessState::Exited { .. })
                ) {
                    return pending("native completion could not be confirmed");
                }
                let mut next = stored.record.clone();
                next.phase = CommandPhase::Canceled;
                match self.command_custody.replace(&stored, next).await {
                    Ok(Some(next)) => return canceled(&next.record),
                    Ok(None) => {}
                    Err(_) => return pending("native completion could not be durably settled"),
                }
            } else {
                return pending("the original native bootstrap has not established exact process custody");
            }
            match self
                .command_custody
                .load(&stored.record.owner, &stored.record.launch_key)
                .await
            {
                Ok(Some(next)) => stored = next,
                _ => return pending("durable command custody became unavailable"),
            }
        }
        pending("command custody is changing concurrently")
    }

    pub async fn handle_cancel_command_launch_with_scope(
        &self,
        sandbox: &LeaseSandbox,
        request: &CancelCommandLaunchRequest,
        scope: &VerifiedVerbScope,
        payload: &[u8],
    ) -> CancelCommandLaunchResponse {
        let subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &request.lease_id);
        match Self::command_owner(sandbox, &request.lease_id, &subject, request, scope, payload) {
            Ok(owner) => self.cancel_controlled(sandbox, request, owner).await,
            Err(error) => CancelCommandLaunchResponse::Denied(error),
        }
    }
}
