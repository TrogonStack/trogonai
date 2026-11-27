use std::fmt::Display;
use std::time::Duration;

use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, ExecutionId, KillCommandRequest, KillCommandResponse,
    LeaseError, LeaseId, ListProcessesRequest, ListProcessesResponse, PollCommandRequest, PollCommandResponse,
    ProcessName, ProcessState, ReadyPorts, RestartCommandRequest, RestartCommandResponse, StdinChunk, StdinEof,
    StdinSeq, StopCommandRequest, StopCommandResponse, UnixSignal, WaitReadyRequest, WaitReadyResponse,
    WaitSandboxReadyRequest, WaitSandboxReadyResponse, WriteStdinRequest, WriteStdinResponse,
};
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient};

/// A request the client refuses to send because it is not valid.
pub(crate) fn invalid_spec(error: impl Display) -> ClientError {
    ClientError::Denied(LeaseError::InvalidSpec {
        reason: error.to_string(),
    })
}

impl SandboxClient {
    /// Polls the status of a previously started detached command (see [`Self::run_command_detached`]).
    pub async fn poll_command(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
    ) -> Result<PollCommandResponse, ClientError> {
        let request = PollCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
        };
        let response: PollCommandResponse = self
            .send_default(&subjects::lease_poll_command(lease_id), &request)
            .await?;
        match response {
            PollCommandResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Lists the named or probed processes started in `lease_id`. [`ListProcessesResponse::Unsupported`] means the
    /// provider does not declare named processes.
    pub async fn list_processes(&self, lease_id: &LeaseId) -> Result<ListProcessesResponse, ClientError> {
        let request = ListProcessesRequest {
            lease_id: lease_id.clone(),
        };
        let response: ListProcessesResponse = self
            .send_default(&subjects::lease_list_processes(lease_id), &request)
            .await?;
        match response {
            ListProcessesResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Waits up to `timeout_seconds` (bounded by the broker) for the readiness probe of a process started with
    /// [`Self::run_process`]. Reports ready, not yet ready, or failed when the process exited first.
    pub async fn wait_ready(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        timeout_seconds: u32,
    ) -> Result<WaitReadyResponse, ClientError> {
        let request = WaitReadyRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            timeout_seconds,
        };
        let bounded = timeout_seconds.min(WaitReadyRequest::MAX_TIMEOUT_SECONDS);
        let timeout = Duration::from_secs(u64::from(bounded) + 15);
        let response: WaitReadyResponse = self
            .send(&subjects::lease_wait_ready(lease_id), &request, timeout)
            .await?;
        match response {
            WaitReadyResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Waits up to `timeout_seconds` (bounded by the broker) for the lease to be running and every port in `ports`
    /// to accept connections inside the sandbox.
    pub async fn wait_sandbox_ready(
        &self,
        lease_id: &LeaseId,
        ports: ReadyPorts,
        timeout_seconds: u32,
    ) -> Result<WaitSandboxReadyResponse, ClientError> {
        let request = WaitSandboxReadyRequest {
            lease_id: lease_id.clone(),
            ports,
            timeout_seconds,
        };
        let bounded = timeout_seconds.min(WaitSandboxReadyRequest::MAX_TIMEOUT_SECONDS);
        // The broker may overshoot its own deadline by one probe command.
        let timeout = Duration::from_secs(u64::from(bounded) + 30);
        let response: WaitSandboxReadyResponse = self
            .send(&subjects::lease_wait_sandbox_ready(lease_id), &request, timeout)
            .await?;
        match response {
            WaitSandboxReadyResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Sends `signal` to the running process named `name` in `lease_id`. [`KillCommandResponse::NotFound`] when no
    /// running process has the name, which makes a repeated call safe.
    pub async fn kill_process_by_name(
        &self,
        lease_id: &LeaseId,
        name: &ProcessName,
        signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError> {
        let ListProcessesResponse::Processes { processes } = self.list_processes(lease_id).await? else {
            return Err(invalid_spec("named processes are not supported by this provider"));
        };
        let running = processes
            .into_iter()
            .find(|process| process.name.as_ref() == Some(name) && process.state == ProcessState::Running);
        match running {
            Some(process) => self.kill_command(lease_id, &process.execution_id, signal).await,
            None => Ok(KillCommandResponse::NotFound),
        }
    }

    /// Writes `data` (and closes stdin when `eof` closes it) to a command started with
    /// [`StdinMode::Interactive`](trogon_sandbox_contract::StdinMode::Interactive). Retrying a failed call with the
    /// same `seq` is safe: the worker applies each `seq` once. An empty write that keeps stdin open is refused
    /// before it is sent.
    pub async fn write_stdin(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        seq: StdinSeq,
        data: StdinChunk,
        eof: StdinEof,
    ) -> Result<WriteStdinResponse, ClientError> {
        let request =
            WriteStdinRequest::new(lease_id.clone(), execution_id.clone(), seq, data, eof).map_err(invalid_spec)?;
        let response: WriteStdinResponse = self
            .send_default(&subjects::lease_write_stdin(lease_id), &request)
            .await?;
        match response {
            WriteStdinResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Cancels a command launch that carries `request.launch_key`.
    pub async fn cancel_command_launch(
        &self,
        request: CancelCommandLaunchRequest,
    ) -> Result<CancelCommandLaunchResponse, ClientError> {
        let response: CancelCommandLaunchResponse = self
            .send_default(&subjects::lease_cancel_command_launch(&request.lease_id), &request)
            .await?;
        match response {
            CancelCommandLaunchResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Sends `signal` to a previously started detached command (see [`Self::run_command_detached`]).
    pub async fn kill_command(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError> {
        let request = KillCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            signal,
        };
        let response: KillCommandResponse = self
            .send_default(&subjects::lease_kill_command(lease_id), &request)
            .await?;
        match response {
            KillCommandResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Gracefully stops a detached command: SIGTERM, wait up to `grace_seconds`, then SIGKILL. Providers without
    /// graceful stop reply [`StopCommandResponse::Unsupported`].
    pub async fn stop_command(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        grace_seconds: Option<u32>,
    ) -> Result<StopCommandResponse, ClientError> {
        let grace_seconds = grace_seconds.unwrap_or(StopCommandRequest::DEFAULT_GRACE_SECONDS);
        let request = StopCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            grace_seconds,
        };
        let timeout = grace_timeout(grace_seconds, 15);
        let response: StopCommandResponse = self
            .send(&subjects::lease_stop_command(lease_id), &request, timeout)
            .await?;
        match response {
            StopCommandResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Stops a detached command and starts it again with the same process spec under a new execution id.
    pub async fn restart_command(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        grace_seconds: Option<u32>,
    ) -> Result<RestartCommandResponse, ClientError> {
        let grace_seconds = grace_seconds.unwrap_or(StopCommandRequest::DEFAULT_GRACE_SECONDS);
        let request = RestartCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: execution_id.clone(),
            grace_seconds,
        };
        let timeout = grace_timeout(grace_seconds, 30);
        let response: RestartCommandResponse = self
            .send(&subjects::lease_restart_command(lease_id), &request, timeout)
            .await?;
        match response {
            RestartCommandResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }
}

/// The reply deadline for a stop or restart: the bounded grace period plus `margin_seconds`.
fn grace_timeout(grace_seconds: u32, margin_seconds: u64) -> Duration {
    let bounded = grace_seconds.min(StopCommandRequest::MAX_GRACE_SECONDS);
    Duration::from_secs(u64::from(bounded) + margin_seconds)
}
