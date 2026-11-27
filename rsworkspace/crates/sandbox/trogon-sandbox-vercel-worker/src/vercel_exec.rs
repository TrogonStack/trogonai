//! Per-lease operations (run, poll, kill, files, domains) over the Vercel session API.
//!
//! Each `handle_*` method takes the wire request and returns the wire response, so the NATS handlers stay thin.
//! Options Vercel cannot serve are refused with the wire's typed outcomes instead of being dropped.
//!
//! A foreground command is one request whose response is an NDJSON stream of `command` records, the last carrying the
//! exit code. The stream holds no output, so the output is read from the command's log stream once it exits. The
//! worker enforces the command timeout itself and kills the command when it elapses. A detached command is started
//! without waiting and polled by id.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::Method;
use serde_json::Value;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    CommandDeadline, CommandExecution, CommandOutput, ExecutionId, KillCommandRequest, KillCommandResponse, LeaseError,
    PollCommandResponse, RunCommandRequest, RunCommandResponse, RunCommandStartedResponse, UnixSignal,
};
use trogon_sandbox_protocol::{
    CappedOutput, effective_max_output_bytes, resolve_exec_cwd, retained_guest_capture_cleanup_script,
    retained_guest_capture_read_script, retained_guest_capture_script,
};

use crate::capture_frame::captured_output;
use crate::collected::Collected;
use crate::constants::{CAPTURE_FRAME_LIMIT, DEFAULT_COMMAND_CWD, LOG_GRACE, SIGKILL_EXIT, STATUS_WAIT};
use crate::drain::drain;
use crate::execution::{CompletionReceipt, Execution, ExecutionState, NativeTerminal};
use crate::json_object::object;
use crate::run_command_violation::run_command_violation;
use crate::safe_id::safe_id;
use crate::signal_number::signal_number;
use crate::{LeaseSandbox, VercelApi, VercelApiError, VercelExecError};

pub(crate) fn denied(error: &impl std::fmt::Display) -> LeaseError {
    LeaseError::Backend {
        message: error.to_string(),
    }
}

pub(crate) fn invalid_spec(reason: impl Into<String>) -> LeaseError {
    LeaseError::InvalidSpec { reason: reason.into() }
}

/// Runs commands, reads and writes files and resolves domains for the leases of one worker.
pub struct VercelExec {
    pub(crate) api: VercelApi,
    pub(crate) executions: Mutex<HashMap<String, Execution>>,
}

impl VercelExec {
    pub fn new(api: VercelApi) -> Self {
        Self {
            api,
            executions: Mutex::new(HashMap::new()),
        }
    }

    /// Forgets every detached command of a lease; called when the lease is released.
    pub async fn forget_lease(&self, lease_id: &str) {
        self.executions.lock().await.retain(|_, e| e.lease_id != lease_id);
    }

    pub(crate) fn session_path(sandbox: &LeaseSandbox, suffix: &str) -> Result<String, VercelApiError> {
        safe_id(&sandbox.session_id, "session id")?;
        Ok(format!("/v2/sandboxes/sessions/{}{suffix}", sandbox.session_id))
    }

    pub(crate) fn captured_command_body(request: &RunCommandRequest, wait: bool, capture_id: &str) -> Value {
        let deadline = match &request.execution {
            CommandExecution::Attached => CommandDeadline::Timeout,
            CommandExecution::Detached { deadline, .. } => *deadline,
        };
        let capture = if deadline == CommandDeadline::Lease {
            format!(
                "capture_without_deadline() {{ shift 3; \"$@\"; }}; {}",
                retained_guest_capture_script("capture_without_deadline", "env")
            )
        } else {
            retained_guest_capture_script("timeout", "env")
        };
        let script = format!("set -- {capture_id} \"$@\"; {capture}");
        let mut args = vec![
            "-c".to_owned(),
            script,
            "sandbox-capture".to_owned(),
            format!("{}s", request.timeout_seconds.max(1)),
            resolve_exec_cwd(DEFAULT_COMMAND_CWD, request.cwd.as_deref()),
        ];
        args.extend(request.env.iter().map(|(name, value)| format!("{name}={value}")));
        args.extend(request.command.iter().cloned());
        let mut body = object([
            ("command", Value::from("sh")),
            ("args", Value::from(args)),
            ("sudo", Value::from(request.run_as_user.is_some())),
        ]);
        if let Some(cwd) = &request.cwd {
            body["cwd"] = Value::from(resolve_exec_cwd(DEFAULT_COMMAND_CWD, Some(cwd)));
        }
        if wait {
            body["wait"] = Value::from(true);
        }
        if deadline == CommandDeadline::Timeout {
            body["timeout"] = Value::from(u64::from(request.timeout_seconds.max(1)) * 1000);
        }
        body
    }

    pub async fn handle_run_command(&self, sandbox: &LeaseSandbox, request: &RunCommandRequest) -> RunCommandResponse {
        if let Some(reason) = run_command_violation(request) {
            return RunCommandResponse::Denied(invalid_spec(reason));
        }
        if matches!(request.execution, CommandExecution::Detached { .. }) {
            return match self.start_detached(sandbox, request).await {
                Ok(execution_id) => RunCommandResponse::Started(RunCommandStartedResponse {
                    origin: Default::default(),
                    execution_id: ExecutionId::new(execution_id),
                }),
                Err(e) => RunCommandResponse::Denied(denied(&e)),
            };
        }
        match self.run_foreground(sandbox, request).await {
            Ok(output) => RunCommandResponse::Completed(output),
            Err(e) => RunCommandResponse::Denied(denied(&e)),
        }
    }

    async fn run_foreground(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
    ) -> Result<CommandOutput, VercelExecError> {
        let path = Self::session_path(sandbox, "/cmd")?;
        let cap = effective_max_output_bytes(request.max_output_bytes, usize::MAX);
        let execution = Execution::new(sandbox, request, cap);
        let mut collected = Collected::with_limit(CAPTURE_FRAME_LIMIT);
        let body = Self::captured_command_body(request, true, &execution.capture_id);
        let limit = Duration::from_secs(u64::from(request.timeout_seconds.max(1)));
        let outcome = tokio::time::timeout(limit, async {
            let response = self
                .api
                .builder(Method::POST, &path, &[])
                .json(&body)
                .send()
                .await
                .map_err(VercelApiError::transport)?;
            let response = VercelApi::check(response).await?;
            drain(response, &mut collected).await
        })
        .await;

        if let Some(id) = &collected.command_id {
            safe_id(id, "command id")?;
            self.executions.lock().await.insert(id.clone(), execution.clone());
        }
        let mut state = execution.state.lock().await;
        let local_deadline_elapsed = outcome.is_err();
        if let Ok(result) = outcome {
            result?;
        }
        let mut timed_out = false;
        if local_deadline_elapsed && collected.exit.is_none() {
            let Some(id) = &collected.command_id else {
                return Err(VercelExecError::CommandIdMissing);
            };
            let kill_accepted = match self.kill(sandbox, id, UnixSignal::Kill).await {
                Ok(()) => true,
                Err(VercelApiError::NotFound(_)) => false,
                Err(source) => {
                    return Err(VercelExecError::DeadlineKillFailed {
                        command_id: id.clone(),
                        source,
                    });
                }
            };
            let exit = self
                .exit_status(sandbox, id)
                .await
                .map_err(|source| VercelExecError::TerminationUnconfirmed {
                    command_id: id.clone(),
                    source,
                })?
                .ok_or_else(|| VercelExecError::NoTerminalReceipt { command_id: id.clone() })?;
            collected.exit = Some(exit);
            // This cause needs both our deadline-triggered SIGKILL and native completion.
            timed_out = kill_accepted && exit == SIGKILL_EXIT;
        }
        let Some(carrier_exit) = collected.exit else {
            let message = collected
                .error
                .take()
                .unwrap_or_else(|| "the command stream ended before an exit status".to_string());
            return Err(VercelExecError::Stream(message));
        };
        let id = collected
            .command_id
            .clone()
            .ok_or(VercelExecError::Capture("native capture lacks command identity"))?;
        state.terminal = Some(NativeTerminal {
            exit: carrier_exit,
            deadline_kill_confirmed: timed_out,
        });
        let output = tokio::time::timeout(LOG_GRACE, async {
            let logs = if carrier_exit == 0 {
                tokio::time::timeout(STATUS_WAIT, self.logs_into(sandbox, &id, &mut collected))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .map(|()| collected)
            } else {
                None
            };
            self.complete_execution(sandbox, &id, &execution, &mut state, logs)
                .await
        })
        .await
        .map_err(|_| VercelExecError::RecoveryTimedOut)??;
        self.executions.lock().await.remove(&id);
        Ok(output)
    }

    async fn start_detached(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
    ) -> Result<String, VercelExecError> {
        let path = Self::session_path(sandbox, "/cmd")?;
        let execution = Execution::new(
            sandbox,
            request,
            effective_max_output_bytes(request.max_output_bytes, usize::MAX),
        );
        let body = Self::captured_command_body(request, false, &execution.capture_id);
        let bytes = self
            .api
            .send(self.api.builder(Method::POST, &path, &[]).json(&body))
            .await?;
        let v: Value = VercelApi::parse(&bytes)?;
        let id = v
            .pointer("/command/id")
            .and_then(Value::as_str)
            .ok_or(VercelExecError::Capture("the response carries no command id"))?
            .to_string();
        safe_id(&id, "command id")?;
        self.executions.lock().await.insert(id.clone(), execution);
        Ok(id)
    }
}

impl VercelExec {
    /// Runs one of the capture scripts that read or remove the retained capture of `execution`.
    async fn capture_control(
        &self,
        sandbox: &LeaseSandbox,
        execution: &Execution,
        script: String,
        status: Option<String>,
    ) -> Result<Collected, VercelExecError> {
        (execution.session_id == sandbox.session_id)
            .then_some(())
            .ok_or(VercelExecError::Capture("capture session ownership changed"))?;
        let mut args = vec![
            "-c".to_owned(),
            script,
            "sandbox-capture-control".to_owned(),
            execution.capture_id.clone(),
        ];
        args.extend(status);
        let body = object([
            ("command", Value::from("sh")),
            ("args", Value::from(args)),
            ("wait", Value::from(true)),
            ("sudo", Value::from(execution.sudo)),
            ("timeout", Value::from(STATUS_WAIT.as_millis() as u64)),
        ]);
        let path = Self::session_path(sandbox, "/cmd")?;
        let response = self
            .api
            .builder(Method::POST, &path, &[])
            .json(&body)
            .send()
            .await
            .map_err(VercelApiError::transport)?;
        let response = VercelApi::check(response).await?;
        let mut collected = Collected::with_limit(CAPTURE_FRAME_LIMIT);
        drain(response, &mut collected).await?;
        if collected.exit != Some(0) || collected.error.is_some() {
            return Err(VercelExecError::Capture(
                "owned capture control did not complete successfully",
            ));
        }
        let id = collected
            .command_id
            .clone()
            .ok_or(VercelExecError::Capture("owned capture control lacks native identity"))?;
        safe_id(&id, "capture control id")?;
        self.logs_into(sandbox, &id, &mut collected).await?;
        if collected.exit != Some(0)
            || collected.command_id.as_deref() != Some(&id)
            || collected.truncated
            || collected.error.is_some()
            || !collected.stderr.into_bytes().is_empty()
        {
            return Err(VercelExecError::Capture("owned capture control output was incomplete"));
        }
        Ok(Collected {
            stderr: CappedOutput::new(0),
            ..collected
        })
    }

    /// Turns the native terminal of a command into its output, recovering the capture when the log stream did not
    /// carry it, and removes the retained capture from the sandbox.
    pub(crate) async fn complete_execution(
        &self,
        sandbox: &LeaseSandbox,
        original_id: &str,
        execution: &Execution,
        state: &mut ExecutionState,
        logs: Option<Collected>,
    ) -> Result<CommandOutput, VercelExecError> {
        let terminal = state.terminal.ok_or(VercelExecError::Capture(
            "capture recovery lacks a native terminal receipt",
        ))?;
        let mut output = if let Some(receipt) = &state.completed {
            let logs = self.logs(sandbox, &receipt.source_id, CAPTURE_FRAME_LIMIT).await?;
            Self::receipt_output(logs, execution, &receipt.source_id)?
        } else {
            let direct = logs
                .filter(|_| terminal.exit == 0)
                .and_then(|logs| Self::receipt_output(logs, execution, original_id).ok());
            let (source_id, output) = match direct {
                Some(output) => (original_id.to_owned(), output),
                None => {
                    let status = if terminal.exit == 0 {
                        "completed".to_owned()
                    } else {
                        terminal.exit.to_string()
                    };
                    let recovered = self
                        .capture_control(sandbox, execution, retained_guest_capture_read_script(), Some(status))
                        .await?;
                    let source_id = recovered.command_id.clone().ok_or(VercelExecError::Capture(
                        "capture read lacks its backing command identity",
                    ))?;
                    let output = Self::receipt_output(recovered, execution, &source_id)?;
                    (source_id, output)
                }
            };
            safe_id(&source_id, "capture receipt source id")?;
            state.completed = Some(CompletionReceipt { source_id });
            state.cleanup_pending = true;
            output
        };
        if terminal.deadline_kill_confirmed {
            output.exit_code = CommandOutput::TIMED_OUT_EXIT_CODE;
            output.timed_out = true.into();
        }
        if state.cleanup_pending {
            let cleanup = self
                .capture_control(sandbox, execution, retained_guest_capture_cleanup_script(), None)
                .await?;
            if !cleanup.stdout.into_bytes().is_empty() {
                return Err(VercelExecError::Capture("capture cleanup returned unexpected output"));
            }
            state.cleanup_pending = false;
        }
        Ok(output)
    }

    fn receipt_output(
        logs: Collected,
        execution: &Execution,
        source_id: &str,
    ) -> Result<CommandOutput, VercelExecError> {
        safe_id(source_id, "capture receipt source id")?;
        if logs.command_id.as_deref().is_some_and(|id| id != source_id) || logs.exit.is_some_and(|exit| exit != 0) {
            return Err(VercelExecError::Capture(
                "capture backing logs name another command or terminal result",
            ));
        }
        captured_output(logs, execution, 0)
    }
}

impl VercelExec {
    /// A timed-out status request means the command is still running; any other transport failure is an error.
    fn timeout_as_none(error: reqwest::Error) -> Result<Option<i32>, VercelApiError> {
        error
            .is_timeout()
            .then_some(None)
            .ok_or_else(|| VercelApiError::transport(error))
    }

    /// `None` while the command runs. Vercel records the exit code only once a caller waits for it, so the status is
    /// read with `wait` and a short deadline: a command still running holds the request open until the deadline
    /// passes.
    pub(crate) async fn exit_status(
        &self,
        sandbox: &LeaseSandbox,
        command_id: &str,
    ) -> Result<Option<i32>, VercelApiError> {
        let path = Self::session_path(sandbox, &format!("/cmd/{command_id}"))?;
        let response = match self
            .api
            .builder(Method::GET, &path, &[("wait", "true")])
            .timeout(STATUS_WAIT)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Self::timeout_as_none(e),
        };
        let response = VercelApi::check(response).await?;
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => return Self::timeout_as_none(e),
        };
        let v: Value = VercelApi::parse(&bytes)?;
        if v.pointer("/command/id").and_then(Value::as_str) != Some(command_id) {
            return Err(VercelApiError::Transport(
                "native terminal receipt names another command".into(),
            ));
        }
        Ok(v.pointer("/command/exitCode")
            .and_then(Value::as_i64)
            .map(|c| i32::try_from(c).unwrap_or(-1)))
    }

    /// Feeds the log stream of a command into `collected`.
    pub(crate) async fn logs_into(
        &self,
        sandbox: &LeaseSandbox,
        command_id: &str,
        collected: &mut Collected,
    ) -> Result<(), VercelExecError> {
        let path = Self::session_path(sandbox, &format!("/cmd/{command_id}/logs"))?;
        let response = self
            .api
            .builder(Method::GET, &path, &[])
            .send()
            .await
            .map_err(VercelApiError::transport)?;
        let response = VercelApi::check(response).await?;
        drain(response, collected).await
    }

    pub(crate) async fn logs(
        &self,
        sandbox: &LeaseSandbox,
        command_id: &str,
        cap: usize,
    ) -> Result<Collected, VercelExecError> {
        let mut collected = Collected::with_limit(cap);
        self.logs_into(sandbox, command_id, &mut collected).await?;
        Ok(collected)
    }

    pub(crate) async fn kill(
        &self,
        sandbox: &LeaseSandbox,
        command_id: &str,
        signal: UnixSignal,
    ) -> Result<(), VercelApiError> {
        let path = Self::session_path(sandbox, &format!("/cmd/{command_id}/kill"))?;
        self.api
            .send(
                self.api
                    .builder(Method::POST, &path, &[])
                    .json(&object([("signal", Value::from(signal_number(signal)))])),
            )
            .await
            .map(|_| ())
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
        let mut state = execution.state.lock().await;
        if state.terminal.is_none() {
            let exit = match self.exit_status(sandbox, execution_id).await {
                Ok(Some(code)) => code,
                Ok(None) => return PollCommandResponse::Running,
                Err(VercelApiError::NotFound(_)) => return PollCommandResponse::NotFound,
                Err(e) => return PollCommandResponse::Denied(denied(&e)),
            };
            state.terminal = Some(NativeTerminal {
                exit,
                deadline_kill_confirmed: false,
            });
        }
        let result = tokio::time::timeout(LOG_GRACE, async {
            let logs = if state.completed.is_none() && state.terminal.is_some_and(|terminal| terminal.exit == 0) {
                tokio::time::timeout(STATUS_WAIT, self.logs(sandbox, execution_id, CAPTURE_FRAME_LIMIT))
                    .await
                    .ok()
                    .and_then(Result::ok)
            } else {
                None
            };
            self.complete_execution(sandbox, execution_id, &execution, &mut state, logs)
                .await
        })
        .await;
        match result {
            Ok(Ok(output)) => PollCommandResponse::Completed(output),
            Ok(Err(error)) => PollCommandResponse::Denied(denied(&error)),
            Err(_) => PollCommandResponse::Denied(denied(&VercelExecError::RecoveryTimedOut)),
        }
    }

    pub async fn handle_kill_command(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        request: &KillCommandRequest,
    ) -> KillCommandResponse {
        let execution_id = request.execution_id.as_str();
        let Some(execution) = self.owned_execution(sandbox, lease_id, execution_id).await else {
            return KillCommandResponse::NotFound;
        };
        let mut state = execution.state.lock().await;
        if state.terminal.is_some() {
            return KillCommandResponse::AlreadyCompleted;
        }
        match self.exit_status(sandbox, execution_id).await {
            Ok(Some(exit)) => {
                state.terminal = Some(NativeTerminal {
                    exit,
                    deadline_kill_confirmed: false,
                });
                return KillCommandResponse::AlreadyCompleted;
            }
            Ok(None) => {}
            Err(VercelApiError::NotFound(_)) => return KillCommandResponse::NotFound,
            Err(e) => return KillCommandResponse::Denied(denied(&e)),
        }
        match self.kill(sandbox, execution_id, request.signal).await {
            Ok(()) => KillCommandResponse::Signaled,
            Err(VercelApiError::NotFound(_)) => KillCommandResponse::NotFound,
            Err(e) => KillCommandResponse::Denied(denied(&e)),
        }
    }

    /// The execution `execution_id` names, when this lease's current session started it.
    async fn owned_execution(
        &self,
        sandbox: &LeaseSandbox,
        lease_id: &str,
        execution_id: &str,
    ) -> Option<crate::execution::Execution> {
        self.executions
            .lock()
            .await
            .get(execution_id)
            .filter(|execution| execution.lease_id == lease_id && execution.session_id == sandbox.session_id)
            .cloned()
    }
}

#[cfg(test)]
mod tests;
