use std::time::Duration;

use tokio::time::Instant;
use trogon_sandbox_contract::{CommandOutput, LeaseError, RunCommandRequest, RunCommandResponse};
use trogon_sandbox_protocol::{GUEST_STREAM_LIMIT, effective_max_output_bytes};

use crate::command_options::{resolve_cwd, run_command_violation};
use crate::constants::{MAX_REFUSED, MAX_RESUMES};
use crate::idempotency::{fresh, idempotency_key};
use crate::{ApiError, CommandBody, CommandInvocation, EventSink, ExecFailureError, SpaceId, SpaceProvider, SseParser};

fn invalid_spec(reason: impl ToString) -> LeaseError {
    LeaseError::InvalidSpec {
        reason: reason.to_string(),
    }
}

impl SpaceProvider {
    /// Runs a command to completion and returns what it printed.
    pub async fn run_command(&self, request: &RunCommandRequest) -> RunCommandResponse {
        if let Some(violation) = run_command_violation(request) {
            return RunCommandResponse::Denied(invalid_spec(violation));
        }
        let Ok(record) = self.record(&request.lease_id).await else {
            return RunCommandResponse::Denied(LeaseError::LeaseNotFound {
                lease_id: request.lease_id.clone(),
            });
        };
        let cwd = match resolve_cwd(record.cwd.as_deref(), request.cwd.as_deref()) {
            Ok(cwd) => cwd,
            Err(violation) => return RunCommandResponse::Denied(invalid_spec(violation)),
        };
        let mut env = record.env.clone();
        env.extend(request.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let key = match &request.launch_key {
            Some(launch) => idempotency_key("command", &[request.lease_id.as_str(), launch.as_str()]),
            None => idempotency_key("command", &[&fresh()]),
        };
        let call = CommandInvocation {
            sandbox: &record.sandbox_id,
            argv: &request.command,
            env: &env,
            cwd: cwd.as_deref(),
            timeout_seconds: request.timeout_seconds,
            cap: effective_max_output_bytes(request.max_output_bytes, GUEST_STREAM_LIMIT),
            key: &key,
        };
        match self.exec_collect(&call).await {
            Ok(output) => RunCommandResponse::Completed(output),
            Err(failure) => RunCommandResponse::Denied(LeaseError::Backend {
                message: failure.to_string(),
            }),
        }
    }

    /// Runs one command and reads its output stream to the exit event.
    ///
    /// The command is created exactly once. Attaching to its event stream is what starts
    /// it, so a failure before the stream opens can still mean a process is running;
    /// that is reported as an unknown outcome and never retried. Only a refused
    /// connection, which sent nothing, is retried. An established stream that drops is
    /// resumed with `Last-Event-ID`.
    pub(crate) async fn exec_collect(&self, call: &CommandInvocation<'_>) -> Result<CommandOutput, ExecFailureError> {
        let body = CommandBody {
            command: call.argv,
            timeout: format!("{}s", call.timeout_seconds),
            env: call.env,
            cwd: call.cwd,
        };
        let created = match self.api.create_command(call.sandbox, &body, call.key).await {
            Ok(created) => created,
            Err(error) if error.definitely_not_applied() => {
                return Err(ExecFailureError::NotStarted(error.to_string()));
            }
            Err(error) => return Err(ExecFailureError::Unknown(error.to_string())),
        };
        let Some((attach, events_url)) = created
            .attach
            .as_ref()
            .and_then(|attach| Some((attach, attach.events_url.as_deref()?)))
        else {
            return Err(ExecFailureError::NotStarted(
                "SPACE returned no event stream for the command".into(),
            ));
        };
        let deadline =
            Instant::now() + Duration::from_secs(u64::from(call.timeout_seconds)) + self.config.command_grace;
        let mut sink = EventSink::new(call.cap);
        let mut attached = false;
        let mut refused = 0;
        let mut resumes = 0;
        loop {
            let opened = tokio::time::timeout_at(
                deadline,
                self.api.open_events(attach, events_url, sink.last_id.as_deref()),
            )
            .await;
            let mut response = match opened {
                Ok(Ok(response)) => response,
                Ok(Err(ApiError::NotApplied(_))) if !attached && refused < MAX_REFUSED => {
                    refused += 1;
                    tokio::time::sleep(self.api.retry_base()).await;
                    continue;
                }
                Ok(Err(error)) if !attached && error.definitely_not_applied() => {
                    return Err(ExecFailureError::NotStarted(error.to_string()));
                }
                Ok(Err(error)) => return Err(ExecFailureError::Unknown(error.to_string())),
                Err(_) => {
                    return Err(ExecFailureError::Unknown(
                        "the event stream did not open before the deadline".into(),
                    ));
                }
            };
            attached = true;
            let mut parser = SseParser::default();
            loop {
                let chunk = match tokio::time::timeout_at(deadline, response.chunk()).await {
                    Err(_) => return Err(self.kill_on_deadline(call.sandbox, &created.process_id).await),
                    Ok(Ok(Some(chunk))) => chunk,
                    Ok(Ok(None) | Err(_)) => break,
                };
                let frames = parser
                    .feed(&chunk)
                    .map_err(|error| ExecFailureError::Unknown(error.to_string()))?;
                for frame in frames {
                    sink.apply(frame)?;
                }
                if let Some(exit) = sink.exit.take() {
                    return Ok(sink.finish(exit));
                }
            }
            resumes += 1;
            if resumes > MAX_RESUMES || parser.is_mid_frame() && sink.last_id.is_none() {
                return Err(ExecFailureError::Unknown(
                    "the event stream ended before the exit event".into(),
                ));
            }
            tokio::time::sleep(self.api.retry_base()).await;
        }
    }

    async fn kill_on_deadline(&self, sandbox: &SpaceId, process: &SpaceId) -> ExecFailureError {
        let cleanup = match self.api.kill_process(sandbox, process).await {
            Ok(()) => "kill accepted without a terminal receipt".to_owned(),
            Err(error) => format!("termination failed: {error}"),
        };
        ExecFailureError::Unknown(format!(
            "native command {sandbox}/{process} did not report its exit before the local deadline; {cleanup}"
        ))
    }
}
