use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    CommandExecution, CommandLaunchOrigin, CommandOutput, ExecutionId, LeaseError, PollCommandResponse, ProcessState,
    ProviderKind, RunCommandRequest, RunCommandResponse, RunCommandStartedResponse,
};
use trogon_sandbox_protocol::{VerifiedVerbScope, subjects};

use super::command_owner::CommandOwner;
use super::constants::{FOREGROUND_POLL_INTERVAL, REMOVE_DIRECTORY_SCRIPT};
use super::e2b_exec::E2bExec;
use super::execution::Execution;
use super::failure::{after_launch, invalid_spec};
use super::launch_release::LaunchRelease;
use super::run_command_violation::run_command_violation;
use crate::LeaseSandbox;

impl E2bExec {
    pub async fn handle_run_command(&self, sandbox: &LeaseSandbox, request: &RunCommandRequest) -> RunCommandResponse {
        self.handle_ordinary_command(sandbox, request, None).await
    }

    async fn handle_ordinary_command(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        owner: Option<&CommandOwner>,
    ) -> RunCommandResponse {
        if let Some(reason) = run_command_violation(request) {
            return RunCommandResponse::Denied(invalid_spec(reason));
        }
        if request.require_launch_cancellation.is_required() {
            return RunCommandResponse::Denied(invalid_spec(
                "launch cancellation requires verified worker assignment authority",
            ));
        }
        if matches!(request.execution, CommandExecution::Detached { .. }) {
            return match self.start_detached(sandbox, request, owner).await {
                Ok(execution_id) => RunCommandResponse::Started(RunCommandStartedResponse {
                    origin: CommandLaunchOrigin::default(),
                    execution_id,
                }),
                Err(error) => RunCommandResponse::Denied(error),
            };
        }
        match self.run_foreground(sandbox, request, owner).await {
            Ok(output) => RunCommandResponse::Completed(output),
            Err(error) => RunCommandResponse::Denied(error),
        }
    }

    pub async fn handle_run_command_with_scope(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        scope: &VerifiedVerbScope,
        payload: &[u8],
    ) -> RunCommandResponse {
        let cancellable = request.require_launch_cancellation.is_required();
        if !cancellable && (request.launch_key.is_none() || !self.command_custody.is_configured()) {
            return self.handle_run_command(sandbox, request).await;
        }
        if let Some(reason) = run_command_violation(request) {
            return RunCommandResponse::Denied(invalid_spec(reason));
        }
        if cancellable
            && (!matches!(request.execution, CommandExecution::Detached { .. }) || request.launch_key.is_none())
        {
            return RunCommandResponse::Denied(invalid_spec(
                "launch cancellation requires a detached command and launch_key",
            ));
        }
        let subject = subjects::provider_run_command(ProviderKind::E2b, &request.lease_id);
        let owner = match Self::command_owner(sandbox, &request.lease_id, &subject, request, scope, payload) {
            Ok(owner) => owner,
            Err(error) => return RunCommandResponse::Denied(error),
        };
        if !cancellable {
            return self.handle_ordinary_command(sandbox, request, Some(&owner)).await;
        }
        match self.start_controlled(sandbox, request, owner).await {
            Ok(execution_id) => RunCommandResponse::Started(RunCommandStartedResponse {
                execution_id,
                origin: CommandLaunchOrigin::Created,
            }),
            Err(error) => RunCommandResponse::Denied(error),
        }
    }

    async fn run_foreground(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        owner: Option<&CommandOwner>,
    ) -> Result<CommandOutput, LeaseError> {
        let execution_id = self.start_detached(sandbox, request, owner).await?;
        self.wait_foreground(sandbox, request, &execution_id)
            .await
            .map_err(after_launch)
    }

    async fn wait_foreground(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        execution_id: &ExecutionId,
    ) -> Result<CommandOutput, LeaseError> {
        loop {
            let gate = self.freeze_executions(request.lease_id.as_str()).await;
            let execution =
                self.execution(&request.lease_id, execution_id)
                    .await
                    .ok_or_else(|| LeaseError::Backend {
                        message: "foreground execution cleanup receipt is no longer held".into(),
                    })?;
            execution.deadline.after_expiry(&self.envd, sandbox, &execution).await?;
            if matches!(
                self.process_state(sandbox, &execution).await?,
                ProcessState::Exited { .. }
            ) {
                let output = match self.poll_execution(sandbox, &request.lease_id, execution_id).await {
                    PollCommandResponse::Completed(output) => output,
                    PollCommandResponse::Denied(error) => return Err(error),
                    PollCommandResponse::Running | PollCommandResponse::NotFound => {
                        return Err(LeaseError::Backend {
                            message: "foreground native end has no settled output receipt".into(),
                        });
                    }
                };
                self.run_script(
                    sandbox,
                    &execution.user,
                    REMOVE_DIRECTORY_SCRIPT,
                    std::slice::from_ref(&execution.dir),
                    &BTreeMap::new(),
                    None,
                )
                .await?;
                let removed = self
                    .executions
                    .lock()
                    .await
                    .remove(&(request.lease_id.clone(), execution_id.clone()));
                drop(gate);
                if let Some(execution) = removed {
                    execution.deadline.stop_supervisor().await;
                }
                return Ok(output);
            }
            drop(gate);
            tokio::time::sleep(FOREGROUND_POLL_INTERVAL).await;
        }
    }

    pub(super) async fn start_detached(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        owner: Option<&CommandOwner>,
    ) -> Result<ExecutionId, LeaseError> {
        let _gate = self.freeze_executions(request.lease_id.as_str()).await;
        if let Some(key) = request.launch_key.as_ref()
            && self.command_custody.is_configured()
        {
            let owner =
                owner.ok_or_else(|| invalid_spec("keyed command launches require verified assignment authority"))?;
            if self.command_custody.load(owner, key).await?.is_some() {
                return Err(invalid_spec(
                    "launch_key already requires retained launch cancellation authority",
                ));
            }
        }
        let execution_id = Execution::fresh_id();
        let execution = self
            .prepare_execution(sandbox, request, &execution_id, LaunchRelease::Immediate)
            .await?;
        self.remember_execution(sandbox, &request.lease_id, &execution_id, execution)
            .await;
        Ok(execution_id)
    }
}
