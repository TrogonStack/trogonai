//! Per-lease commands over the Daytona toolbox.
//!
//! Each `handle_*` method takes the wire request and returns the wire response, so the NATS handlers stay thin. The
//! toolbox answers a command call with an exit code and one merged stream, so every command sends its stdout and
//! stderr to files in the sandbox and the worker reads them back. A foreground command is one call. A detached
//! command runs in a session of its own, writes its exit status last, and is polled by reading the files.

mod ordinary;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    CommandDeadline, CommandExecution, CommandOutput, CommandTimeoutStatus, ExecutionId, KillCommandRequest,
    KillCommandResponse, LeaseError, PollCommandResponse, RunCommandRequest, RunCommandResponse,
    RunCommandStartedResponse, StdinMode,
};
use trogon_sandbox_protocol::{cap_buffered, effective_max_output_bytes};

use crate::constants::{COMPLETED_RESULT_TTL, MAX_COMPLETED_RESULTS, TOOLBOX_DEADLINE_SLACK};
use crate::execute_request::ExecuteRequest;
use crate::execution::{Execution, evict_completed};
use crate::ordinary_signal_outcome::OrdinarySignalOutcome;
use crate::run_command_violation::run_command_violation;
use crate::scripts::{capture_output, closed_stdin, interactive_stdin, render_request, with_deadline};
use crate::session_command::SessionCommand;
use crate::side_files::SideFiles;
use crate::signal_number::signal_number;
use crate::stdin_state::StdinState;
use crate::{DaytonaApi, DaytonaApiError, DaytonaExecError, LeaseSandbox, OrdinaryCommandRuntime};

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
    pub(crate) runtime: Option<Arc<OrdinaryCommandRuntime>>,
}

impl DaytonaExec {
    pub fn new(api: DaytonaApi) -> Self {
        Self {
            api,
            executions: Mutex::new(HashMap::new()),
            result_ttl: COMPLETED_RESULT_TTL,
            runtime: None,
        }
    }

    /// Keeps the result of a finished detached command for `ttl` instead of the default.
    #[must_use]
    pub fn with_result_ttl(mut self, ttl: Duration) -> Self {
        self.result_ttl = ttl;
        self
    }

    /// Runs detached commands under the verified runtime, which makes them signalable and lets them take stdin
    /// without a session pipe.
    #[must_use]
    pub fn with_runtime(mut self, runtime: OrdinaryCommandRuntime) -> Self {
        self.runtime = Some(Arc::new(runtime));
        self
    }

    /// Forgets every detached command of a lease; called when the lease is released.
    pub async fn forget_lease(&self, lease_id: &str) {
        self.executions
            .lock()
            .await
            .retain(|_, execution| execution.lease_id != lease_id);
    }

    pub async fn handle_run_command(&self, sandbox: &LeaseSandbox, request: &RunCommandRequest) -> RunCommandResponse {
        if let Some(reason) = run_command_violation(request) {
            return RunCommandResponse::Denied(invalid_spec(reason));
        }
        let line = render_request(request);
        let CommandExecution::Detached { stdin, deadline, .. } = &request.execution else {
            return match self.run_attached(sandbox, request, &line).await {
                Ok(output) => RunCommandResponse::Completed(output),
                Err(error) => RunCommandResponse::Denied(error.into_lease_error()),
            };
        };
        match self.start_detached(sandbox, request, &line, *stdin, *deadline).await {
            Ok(execution_id) => RunCommandResponse::Started(RunCommandStartedResponse {
                origin: Default::default(),
                execution_id: ExecutionId::new(execution_id),
            }),
            Err(error) => RunCommandResponse::Denied(error.into_lease_error()),
        }
    }

    async fn run_attached(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        line: &str,
    ) -> Result<CommandOutput, DaytonaExecError> {
        let files = SideFiles::new(&uuid::Uuid::new_v4().simple().to_string());
        let seconds = request.timeout_seconds.max(1);
        let command = capture_output(
            &with_deadline(line, seconds, &files.status, StdinMode::Closed),
            &files.out,
            &files.err,
        );
        let reply = self
            .api
            .execute(
                &sandbox.id,
                &ExecuteRequest {
                    command,
                    cwd: None,
                    timeout: Some(u64::from(seconds) + TOOLBOX_DEADLINE_SLACK.as_secs()),
                },
            )
            .await?;
        let timed_out = self.deadline_status(&sandbox.id, &files.status).await?;
        let cap = effective_max_output_bytes(request.max_output_bytes, usize::MAX);
        let output = self
            .read_output(&sandbox.id, &files, cap, reply.exit_code, timed_out)
            .await?;
        for path in [&files.out, &files.err, &files.status] {
            let _ = self.api.delete_file(&sandbox.id, path).await;
        }
        Ok(output)
    }

    /// Reads the deadline receipt of a command; `TimedOut` when the deadline ended it.
    async fn deadline_status(&self, id: &str, path: &str) -> Result<CommandTimeoutStatus, DaytonaExecError> {
        let bytes = self
            .api
            .download(id, path)
            .await
            .map_err(DaytonaExecError::ReceiptUnreadable)?;
        match &bytes[..] {
            b"completed" => Ok(CommandTimeoutStatus::WithinDeadline),
            b"timed_out" => Ok(CommandTimeoutStatus::TimedOut),
            b"unavailable" => Err(DaytonaExecError::TimeoutUnavailable),
            _ => Err(DaytonaExecError::ReceiptInvalid),
        }
    }

    async fn read_stream(&self, id: &str, path: &str, stream: &'static str) -> Result<Vec<u8>, DaytonaExecError> {
        self.api
            .download(id, path)
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|_| DaytonaExecError::StreamUnreadable { stream })
    }

    /// The output of a command that ended: both streams, capped, with the exit code a deadline kill replaces.
    async fn read_output(
        &self,
        id: &str,
        files: &SideFiles,
        cap: usize,
        exit_code: i32,
        timed_out: CommandTimeoutStatus,
    ) -> Result<CommandOutput, DaytonaExecError> {
        let stdout = self.read_stream(id, &files.out, "stdout").await?;
        let stderr = self.read_stream(id, &files.err, "stderr").await?;
        let (stdout, out_truncated) = cap_buffered(stdout, cap);
        let (stderr, err_truncated) = cap_buffered(stderr, cap);
        Ok(CommandOutput {
            exit_code: if timed_out.is_timed_out() {
                CommandOutput::TIMED_OUT_EXIT_CODE
            } else {
                exit_code
            },
            stdout,
            stderr,
            timed_out,
            truncated: (out_truncated.is_truncated() || err_truncated.is_truncated()).into(),
        })
    }

    async fn start_detached(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        line: &str,
        stdin: StdinMode,
        deadline: CommandDeadline,
    ) -> Result<String, DaytonaExecError> {
        let execution_id = uuid::Uuid::new_v4().simple().to_string();
        let session_id = format!("trogon-{execution_id}");
        let files = SideFiles::new(&execution_id);
        let interactive = stdin == StdinMode::Interactive;
        if let Some(runtime) = self.runtime.clone() {
            let ordinary = self
                .start_ordinary(&runtime, sandbox, request, &execution_id, &session_id, stdin, deadline)
                .await?;
            let execution = Execution {
                lease_id: request.lease_id.as_str().to_owned(),
                sandbox_id: sandbox.id.clone(),
                session_id,
                command_id: ordinary.command_id.clone(),
                files,
                max_output_bytes: effective_max_output_bytes(request.max_output_bytes, usize::MAX),
                has_deadline: false,
                stdin: interactive.then(|| Arc::new(StdinState::default())),
                ordinary: Some(ordinary),
                completed: None,
                completed_at: None,
            };
            let mut executions = self.executions.lock().await;
            evict_completed(&mut executions, Instant::now(), self.result_ttl, MAX_COMPLETED_RESULTS);
            executions.insert(execution_id.clone(), execution);
            return Ok(execution_id);
        }
        // A non-interactive command must not read the session's own input pipe, which stays open for the session's
        // lifetime and would block a command waiting on end of input.
        let line = if interactive {
            interactive_stdin(line, &files.stdin)
        } else {
            closed_stdin(line)
        };
        let has_deadline = deadline == CommandDeadline::Timeout;
        let line = if has_deadline {
            with_deadline(&line, request.timeout_seconds, &files.status, stdin)
        } else {
            line
        };
        let line = format!(
            "{}; echo $? > {}",
            capture_output(&line, &files.out, &files.err),
            crate::shell::quote(&files.exit)
        );
        self.api.create_session(&sandbox.id, &session_id).await?;
        let started = self
            .api
            .start_session_command(
                &sandbox.id,
                &session_id,
                &SessionCommand {
                    command: line,
                    run_async: true,
                    suppress_input_echo: interactive.then_some(true),
                },
            )
            .await;
        let command_id = match started {
            Ok(Some(command_id)) => command_id,
            Ok(None) => {
                let _ = self.api.delete_session(&sandbox.id, &session_id).await;
                return Err(DaytonaExecError::NoCommandId);
            }
            Err(error) => {
                let _ = self.api.delete_session(&sandbox.id, &session_id).await;
                return Err(error.into());
            }
        };
        let execution = Execution {
            lease_id: request.lease_id.as_str().to_owned(),
            sandbox_id: sandbox.id.clone(),
            session_id,
            command_id,
            files,
            max_output_bytes: effective_max_output_bytes(request.max_output_bytes, usize::MAX),
            has_deadline,
            stdin: interactive.then(|| Arc::new(StdinState::default())),
            ordinary: None,
            completed: None,
            completed_at: None,
        };
        let mut executions = self.executions.lock().await;
        evict_completed(&mut executions, Instant::now(), self.result_ttl, MAX_COMPLETED_RESULTS);
        executions.insert(execution_id.clone(), execution);
        Ok(execution_id)
    }

    /// `None` while the command has not written its exit status yet.
    pub(crate) async fn exit_status(&self, execution: &Execution) -> Result<Option<i32>, DaytonaExecError> {
        if let Some(output) = &execution.completed {
            return Ok(Some(output.exit_code));
        }
        if let Some(ordinary) = &execution.ordinary {
            return Ok(ordinary
                .finished(&self.api)
                .await?
                .map(|completion| completion.root_exit().code()));
        }
        match self.api.download(&execution.sandbox_id, &execution.files.exit).await {
            Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).trim().parse().ok()),
            Err(DaytonaApiError::NotFound(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
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
        if let Some(output) = execution.completed {
            return PollCommandResponse::Completed(output);
        }
        if let Some(ordinary) = execution.ordinary.clone() {
            return self.poll_ordinary(&execution, &ordinary, execution_id).await;
        }
        let exit_code = match self.exit_status(&execution).await {
            Ok(Some(code)) => code,
            Ok(None) => return PollCommandResponse::Running,
            Err(error) => return PollCommandResponse::Denied(error.into_lease_error()),
        };
        let timed_out = if execution.has_deadline {
            match self
                .deadline_status(&execution.sandbox_id, &execution.files.status)
                .await
            {
                Ok(status) => status,
                Err(error) => return PollCommandResponse::Denied(error.into_lease_error()),
            }
        } else {
            CommandTimeoutStatus::WithinDeadline
        };
        let output = match self
            .read_output(
                &execution.sandbox_id,
                &execution.files,
                execution.max_output_bytes,
                exit_code,
                timed_out,
            )
            .await
        {
            Ok(output) => output,
            Err(error) => return PollCommandResponse::Denied(error.into_lease_error()),
        };
        self.reap(&execution, execution_id, &output).await;
        PollCommandResponse::Completed(output)
    }

    /// Keeps a finished command's result in memory and removes what it left in the sandbox: its side files and its
    /// session. Best effort, so a failed removal only leaves the files behind.
    pub(crate) async fn reap(&self, execution: &Execution, execution_id: &str, output: &CommandOutput) {
        {
            let mut executions = self.executions.lock().await;
            let now = Instant::now();
            if let Some(entry) = executions.get_mut(execution_id) {
                entry.completed = Some(output.clone());
                entry.completed_at = Some(now);
            }
            evict_completed(&mut executions, now, self.result_ttl, MAX_COMPLETED_RESULTS);
        }
        if let Some(ordinary) = &execution.ordinary {
            ordinary.cleanup(&self.api).await;
        }
        for path in execution.files.all() {
            let _ = self.api.delete_file(&execution.sandbox_id, path).await;
        }
        let _ = self
            .api
            .delete_session(&execution.sandbox_id, &execution.session_id)
            .await;
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
        if execution.completed.is_some() {
            return KillCommandResponse::AlreadyCompleted;
        }
        if matches!(self.exit_status(&execution).await, Ok(Some(_))) {
            return KillCommandResponse::AlreadyCompleted;
        }
        let Some(ordinary) = &execution.ordinary else {
            return KillCommandResponse::Denied(denied(&DaytonaExecError::SignalNeedsRuntime));
        };
        match ordinary.signal(&self.api, signal_number(request.signal)).await {
            Ok(OrdinarySignalOutcome::Delivered) => KillCommandResponse::Signaled,
            Ok(OrdinarySignalOutcome::AlreadyCompleted) => KillCommandResponse::AlreadyCompleted,
            Err(error) => KillCommandResponse::Denied(denied(&error)),
        }
    }

    /// The execution `execution_id` names, when this lease's sandbox started it.
    pub(crate) async fn owned_execution(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        execution_id: &str,
    ) -> Option<Execution> {
        self.executions
            .lock()
            .await
            .get(execution_id)
            .filter(|execution| execution.lease_id == lease_id && execution.sandbox_id == sandbox.id)
            .cloned()
    }
}
