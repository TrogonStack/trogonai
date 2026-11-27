//! Per-lease commands over the Daytona toolbox.
//!
//! Each `handle_*` method takes the wire request and returns the wire response, so the NATS handlers stay thin. The
//! toolbox answers a command call with the exit code and the merged output, so the worker runs every command inside a
//! capture script that keeps stdout and stderr apart and prints them in one frame. A foreground command is one
//! call. A detached command is started in the background of the sandbox, writes its frame and a done marker under a
//! directory of its own, and is polled by reading them.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    CommandDeadline, CommandExecution, CommandOutput, ExecutionId, KillCommandRequest, KillCommandResponse, LeaseError,
    PollCommandResponse, RunCommandRequest, RunCommandResponse, RunCommandStartedResponse,
};
use trogon_sandbox_protocol::{effective_max_output_bytes, parse_guest_capture};

use crate::constants::{COMPLETED_RESULT_TTL, RUNNING_EXIT, TOOLBOX_DEADLINE_SLACK};
use crate::execute_request::ExecuteRequest;
use crate::execution::{Execution, Finished};
use crate::run_command_violation::run_command_violation;
use crate::scripts::{
    capture_words, command_dir, detach_command, join_quoted, marker_line, poll_script, remove_script, signal_script,
};
use crate::signal_number::signal_number;
use crate::{DaytonaApi, DaytonaApiError, DaytonaExecError, LeaseSandbox};

pub(crate) fn denied(error: &impl std::fmt::Display) -> LeaseError {
    LeaseError::Backend {
        message: error.to_string(),
    }
}

pub(crate) fn invalid_spec(reason: impl Into<String>) -> LeaseError {
    LeaseError::InvalidSpec { reason: reason.into() }
}

/// Runs commands, reads and writes files and resolves domains for the leases of one worker.
pub struct DaytonaExec {
    pub(crate) api: DaytonaApi,
    pub(crate) executions: Mutex<HashMap<String, Execution>>,
    result_ttl: Duration,
}

impl DaytonaExec {
    pub fn new(api: DaytonaApi) -> Self {
        Self {
            api,
            executions: Mutex::new(HashMap::new()),
            result_ttl: COMPLETED_RESULT_TTL,
        }
    }

    /// Keeps the result of a finished detached command for `ttl` instead of the default.
    #[must_use]
    pub fn with_result_ttl(mut self, ttl: Duration) -> Self {
        self.result_ttl = ttl;
        self
    }

    /// Forgets every detached command of a lease; called when the lease is released.
    pub async fn forget_lease(&self, lease_id: &str) {
        self.executions.lock().await.retain(|_, e| e.lease_id != lease_id);
    }

    pub async fn handle_run_command(&self, sandbox: &LeaseSandbox, request: &RunCommandRequest) -> RunCommandResponse {
        if let Some(reason) = run_command_violation(request) {
            return RunCommandResponse::Denied(invalid_spec(reason));
        }
        if let CommandExecution::Detached { deadline, .. } = &request.execution {
            return match self.start_detached(sandbox, request, *deadline).await {
                Ok(execution_id) => RunCommandResponse::Started(RunCommandStartedResponse {
                    origin: Default::default(),
                    execution_id: ExecutionId::new(execution_id),
                }),
                Err(error) => RunCommandResponse::Denied(denied(&error)),
            };
        }
        match self.run_foreground(sandbox, request).await {
            Ok(output) => RunCommandResponse::Completed(output),
            Err(error) => RunCommandResponse::Denied(denied(&error)),
        }
    }

    async fn run_shell(
        &self,
        sandbox: &LeaseSandbox,
        command: String,
        seconds: u64,
    ) -> Result<(i32, String), DaytonaApiError> {
        let reply = self
            .api
            .execute(
                &sandbox.id,
                &ExecuteRequest {
                    command,
                    cwd: None,
                    timeout: Some(seconds),
                },
            )
            .await?;
        Ok((reply.exit_code, reply.result))
    }

    async fn run_foreground(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
    ) -> Result<CommandOutput, DaytonaExecError> {
        let seconds = request.timeout_seconds.max(1);
        let cap = effective_max_output_bytes(request.max_output_bytes, usize::MAX);
        let command = join_quoted(&capture_words(request, CommandDeadline::Timeout, None));
        let limit = u64::from(seconds) + TOOLBOX_DEADLINE_SLACK.as_secs();
        let (code, frame) = self.run_shell(sandbox, command, limit).await?;
        if code != 0 {
            return Err(DaytonaExecError::ShellFailed(code));
        }
        Ok(parse_guest_capture(&frame, seconds, cap)?)
    }

    async fn start_detached(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        deadline: CommandDeadline,
    ) -> Result<String, DaytonaExecError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let dir = command_dir(&id);
        let capture = join_quoted(&capture_words(request, deadline, Some(&id)));
        let (code, _) = self
            .run_shell(
                sandbox,
                detach_command(&dir, &capture),
                TOOLBOX_DEADLINE_SLACK.as_secs(),
            )
            .await?;
        if code != 0 {
            return Err(DaytonaExecError::ShellFailed(code));
        }
        let execution = Execution {
            lease_id: request.lease_id.as_str().to_owned(),
            sandbox_id: sandbox.id.clone(),
            dir,
            marker: marker_line(&id),
            max_output_bytes: effective_max_output_bytes(request.max_output_bytes, usize::MAX),
            timeout_seconds: request.timeout_seconds.max(1),
            finished: Default::default(),
        };
        let mut executions = self.executions.lock().await;
        executions.retain(|_, held| !held.expired(self.result_ttl));
        executions.insert(id.clone(), execution);
        Ok(id)
    }

    pub async fn handle_poll_command(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        execution_id: &str,
    ) -> PollCommandResponse {
        let Some(execution) = self.owned_execution(sandbox, lease_id, execution_id).await else {
            return PollCommandResponse::NotFound;
        };
        let mut finished = execution.finished.lock().await;
        if let Some(done) = finished.as_ref() {
            return PollCommandResponse::Completed(done.output.clone());
        }
        let script = poll_script(&execution.dir, RUNNING_EXIT);
        let (code, frame) = match self.run_shell(sandbox, script, TOOLBOX_DEADLINE_SLACK.as_secs()).await {
            Ok(reply) => reply,
            Err(error) => return PollCommandResponse::Denied(denied(&error)),
        };
        if code == RUNNING_EXIT {
            return PollCommandResponse::Running;
        }
        if code != 0 {
            return PollCommandResponse::Denied(denied(&DaytonaExecError::ShellFailed(code)));
        }
        let output = match parse_guest_capture(&frame, execution.timeout_seconds, execution.max_output_bytes) {
            Ok(output) => output,
            Err(error) => return PollCommandResponse::Denied(denied(&DaytonaExecError::Frame(error))),
        };
        *finished = Some(Finished {
            output: output.clone(),
            at: std::time::Instant::now(),
        });
        // The result is kept in memory now; a failed cleanup only leaves a small directory in the sandbox.
        let _ = self
            .run_shell(sandbox, remove_script(&execution.dir), TOOLBOX_DEADLINE_SLACK.as_secs())
            .await;
        PollCommandResponse::Completed(output)
    }

    pub async fn handle_kill_command(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        request: &KillCommandRequest,
    ) -> KillCommandResponse {
        let Some(execution) = self
            .owned_execution(sandbox, lease_id, request.execution_id.as_str())
            .await
        else {
            return KillCommandResponse::NotFound;
        };
        if execution.finished.lock().await.is_some() {
            return KillCommandResponse::AlreadyCompleted;
        }
        let signal = signal_script(&execution.marker, signal_number(request.signal));
        let slack = TOOLBOX_DEADLINE_SLACK.as_secs();
        match self.run_shell(sandbox, signal, slack).await {
            Ok((0, _)) => KillCommandResponse::Signaled,
            Ok((1, _)) => {
                match self
                    .run_shell(sandbox, poll_script(&execution.dir, RUNNING_EXIT), slack)
                    .await
                {
                    Ok((0, _)) => KillCommandResponse::AlreadyCompleted,
                    Ok(_) => KillCommandResponse::Denied(denied(&DaytonaExecError::NoProcess)),
                    Err(error) => KillCommandResponse::Denied(denied(&error)),
                }
            }
            Ok((code, _)) => KillCommandResponse::Denied(denied(&DaytonaExecError::ShellFailed(code))),
            Err(error) => KillCommandResponse::Denied(denied(&error)),
        }
    }

    /// The execution `execution_id` names, when this lease's sandbox started it.
    async fn owned_execution(&self, sandbox: &LeaseSandbox, lease_id: &str, execution_id: &str) -> Option<Execution> {
        self.executions
            .lock()
            .await
            .get(execution_id)
            .filter(|execution| execution.lease_id == lease_id && execution.sandbox_id == sandbox.id)
            .cloned()
    }
}
