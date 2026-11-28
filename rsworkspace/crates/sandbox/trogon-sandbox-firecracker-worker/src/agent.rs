//! The worker's side of the in-guest agent's HTTP contract, carried over vsock.
//!
//! The guest has no network device, so the only way a command reaches it is the vsock
//! listener the agent opens. Each call dials a fresh vsock stream (see [`VsockEndpoint`]) and
//! speaks plain HTTP/1.1 over it, the same contract other workers speak through a CONNECT
//! tunnel.
//!
//! Nothing here decides policy. The agent already owns validation, detached execution,
//! signal delivery and lease-mismatch refusal, so a `Denied` coming back over HTTP is a
//! verdict to forward, not an error to translate.

mod constants;
mod terminal_client;
mod transport;
mod trust;
pub use terminal_client::TerminalStream;
pub use trust::GuestProxyTrust;

use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::Digest as _;
use tracing::warn;
use trogon_sandbox_actor_agent::{
    ActorIdentity, BIND_PATH, BindLeaseRequest, BindLeaseResponse, EXEC_KILL_PATH, EXEC_PATH, EXEC_POLL_PATH,
    EXEC_WRITE_STDIN_PATH, ExecFrame, FILES_READ_PATH, FILES_WRITE_PATH, GROUP_MEMBERSHIP_PATH, GROUPS_CREATE_PATH,
    GuestEntropyReseedMethod, GuestProtocolError, IDENTITY_PATH, MIN_AGENT_PROTOCOL_VERSION_ENV,
    MIN_SUPPORTED_AGENT_PROTOCOL_VERSION, MinProtocolVersionError, PROTOCOL_REFUSED_STATUS, READYZ_PATH,
    USERS_CREATE_PATH, try_min_protocol_version_from_env,
};
use trogon_sandbox_contract::{
    CommandEnv, CommandExecution, CommandOutputChunk, CreateSandboxGroupRequest, CreateSandboxGroupResponse,
    CreateSandboxUserRequest, CreateSandboxUserResponse, ExecutionId, KillCommandRequest, KillCommandResponse,
    LaunchCancellation, LeaseId, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, OutputRetention,
    PollCommandRequest, PollCommandResponse, ReadFileRequest, ReadFileResponse, RedactValues, RunCommandRequest,
    RunCommandResponse, StreamedOutput, WriteFilesRequest, WriteFilesResponse, WriteStdinRequest, WriteStdinResponse,
};
use trogon_std::env::ReadEnv;

use constants::{HOST, PROBE_TIMEOUT};

use crate::vsock::VsockEndpoint;

/// Bounds on one whole call, from dialing the stream to the last byte of the body, so a guest
/// that stops answering mid-response cannot hold a request forever.
#[derive(Debug, Clone, Copy)]
pub struct AgentTimeouts {
    pub control: Duration,
    /// Extra wall-clock allowed on top of a command's own timeout. The agent enforces
    /// `timeout_seconds` itself and answers with the partial output, which is strictly better
    /// than a truncated body, so this only stops a wedged guest.
    pub exec_margin: Duration,
}

impl Default for AgentTimeouts {
    fn default() -> Self {
        Self {
            control: Duration::from_secs(30),
            exec_margin: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// The guest never answered. Usually means it is not up yet.
    #[error("agent at {target} is unreachable: {message}")]
    Unreachable { target: String, message: String },
    /// The agent answered with a status its contract does not define.
    #[error("agent at {target} returned HTTP {status}: {body}")]
    Http { target: String, status: u16, body: String },
    /// The body did not parse. A version skew between this worker and the guest rootfs.
    #[error("agent at {target} returned an unreadable body: {message}")]
    Malformed { target: String, message: String },
    /// The agent did not become ready inside the deadline.
    #[error("agent at {target} was not ready within {seconds}s")]
    NotReady { target: String, seconds: u64 },
    /// The call started but the guest stopped answering before it finished.
    #[error("agent at {target} did not answer within {after:?}")]
    Unresponsive { target: String, after: Duration },
    /// The agent refused this worker's protocol version, or speaks one older than this worker
    /// still drives. Retrying cannot help: the guest image or the worker must be upgraded, or
    /// the minimum lowered for the rollout (`SANDBOX_MIN_AGENT_PROTOCOL_VERSION` here,
    /// `SANDBOX_MIN_WORKER_PROTOCOL_VERSION` on the agent).
    #[error("agent at {target} is on an incompatible protocol: {reason}")]
    ProtocolMismatch { target: String, reason: GuestProtocolError },
}

/// Where live output chunks go while a command is still running.
///
/// The subject depends on the execution id, which the *agent* mints, so the sink derives it
/// per chunk instead of being handed a fixed subject at construction.
#[async_trait]
pub trait ChunkSink: Send + Sync {
    async fn publish(&self, chunk: CommandOutputChunk);

    /// Learns the execution's id before any output, so a command that prints nothing can
    /// still be named. Agents that predate the begin frame never call this.
    async fn begin(&self, _execution_id: ExecutionId) {}
}

/// Drops every chunk. Used when the request did not ask for streaming.
pub struct DiscardChunks;

#[async_trait]
impl ChunkSink for DiscardChunks {
    async fn publish(&self, _chunk: CommandOutputChunk) {}
}

/// A client for one guest's HTTP contract.
#[derive(Debug, Clone)]
pub struct GuestAgent {
    endpoint: VsockEndpoint,
    timeouts: AgentTimeouts,
    min_agent_protocol_version: u32,
}

impl GuestAgent {
    pub fn new(endpoint: VsockEndpoint) -> Self {
        Self {
            endpoint,
            timeouts: AgentTimeouts::default(),
            min_agent_protocol_version: MIN_SUPPORTED_AGENT_PROTOCOL_VERSION,
        }
    }

    /// Applies the oldest agent protocol named by [`MIN_AGENT_PROTOCOL_VERSION_ENV`] in `env`,
    /// keeping the current minimum when it is unset. An override that is set but is not an
    /// unsigned integer is refused.
    pub fn with_env(mut self, env: &impl ReadEnv) -> Result<Self, MinProtocolVersionError> {
        self.min_agent_protocol_version = try_min_protocol_version_from_env(
            env,
            MIN_AGENT_PROTOCOL_VERSION_ENV,
            MIN_SUPPORTED_AGENT_PROTOCOL_VERSION,
        )?;
        Ok(self)
    }

    /// Overrides the oldest agent protocol this client drives (zero admits guest images that
    /// predate the handshake).
    pub fn with_min_agent_protocol_version(mut self, version: u32) -> Self {
        self.min_agent_protocol_version = version;
        self
    }

    pub fn with_timeouts(mut self, timeouts: AgentTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Blocks until the agent's HTTP server answers, or the deadline passes.
    ///
    /// The vsock socket exists as soon as the VMM starts, but connecting only succeeds once
    /// the guest kernel is up and the agent is listening, so a failed probe here is the
    /// normal state for the first part of a boot.
    pub async fn wait_ready(&self, deadline: Duration) -> Result<(), AgentError> {
        tokio::time::timeout(deadline, self.wait_ready_inner())
            .await
            .map_err(|_| AgentError::NotReady {
                target: self.endpoint.to_string(),
                seconds: deadline.as_secs(),
            })?
    }

    async fn wait_ready_inner(&self) -> Result<(), AgentError> {
        let mut backoff = Duration::from_millis(50);
        loop {
            match self.within(PROBE_TIMEOUT, self.get(READYZ_PATH)).await {
                Ok(response) if response.status().is_success() => return Ok(()),
                // Waiting longer cannot fix a version the worker will not drive.
                Err(error @ AgentError::ProtocolMismatch { .. }) => return Err(error),
                _ => {}
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_millis(500));
        }
    }

    /// Tells the guest which lease it serves. Must precede any exec: the agent refuses
    /// commands for a lease it was not bound to.
    pub async fn bind(&self, request: &BindLeaseRequest) -> Result<(), AgentError> {
        self.within(self.timeouts.control, async {
            let response = self.send(BIND_PATH, request).await?;
            let response = self.check_status(response).await?;
            if let Some(seed) = &request.entropy_seed {
                let receipt: BindLeaseResponse = self.decode(response).await?;
                let expected = seed_digest(seed.as_bytes()).map_err(|e| AgentError::Malformed {
                    target: self.endpoint.to_string(),
                    message: e.to_string(),
                })?;
                if receipt.lease_id != request.lease_id
                    || receipt.entropy_reseed.seed_digest != expected
                    || receipt.entropy_reseed.method != GuestEntropyReseedMethod::LinuxKernelV1
                {
                    return Err(AgentError::Malformed {
                        target: self.endpoint.to_string(),
                        message: "guest did not acknowledge kernel reseed for this lease and seed".into(),
                    });
                }
            }
            Ok(())
        })
        .await
    }

    pub async fn identity(&self) -> Result<ActorIdentity, AgentError> {
        self.within(self.timeouts.control, async {
            let response = self.get(IDENTITY_PATH).await?;
            let response = self.check_status(response).await?;
            self.decode(response).await
        })
        .await
    }

    /// Runs a command, forwarding every chunk to `sink` as it arrives.
    ///
    /// The body is newline-delimited and ends with exactly one response frame. A stream that
    /// ends without one is reported as malformed rather than as a success with empty output,
    /// because "the command produced nothing" and "the connection died mid-command" must not
    /// look the same to a caller deciding whether work happened.
    pub async fn exec(
        &self,
        request: &RunCommandRequest,
        sink: &dyn ChunkSink,
    ) -> Result<RunCommandResponse, AgentError> {
        let timeout = Duration::from_secs(u64::from(request.timeout_seconds)) + self.timeouts.exec_margin;
        self.within(timeout, self.exec_stream(request, sink)).await
    }

    async fn exec_stream(
        &self,
        request: &RunCommandRequest,
        sink: &dyn ChunkSink,
    ) -> Result<RunCommandResponse, AgentError> {
        let response = self.send(EXEC_PATH, request).await?;
        let response = self.check_status(response).await?;

        let mut body = response.into_body();
        let mut buffer = Vec::new();
        let mut outcome = None;

        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| self.unreachable(e.to_string()))?;
            let Ok(data) = frame.into_data() else {
                continue;
            };
            buffer.extend_from_slice(&data);

            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=end).collect();
                if let Some(frame) = self.decode_frame(&line[..line.len() - 1]) {
                    match frame {
                        ExecFrame::Begin(begin) => sink.begin(begin.begin_execution_id).await,
                        ExecFrame::Chunk(chunk) => sink.publish(chunk).await,
                        ExecFrame::Response(response) => outcome = Some(response),
                    }
                }
            }
        }

        if !buffer.is_empty()
            && let Some(ExecFrame::Response(response)) = self.decode_frame(&buffer)
        {
            outcome = Some(response);
        }

        outcome.ok_or_else(|| AgentError::Malformed {
            target: self.endpoint.to_string(),
            message: "exec stream ended without a response frame".to_string(),
        })
    }

    pub async fn poll(&self, request: &PollCommandRequest) -> Result<PollCommandResponse, AgentError> {
        self.call(EXEC_POLL_PATH, request).await
    }

    pub async fn kill(&self, request: &KillCommandRequest) -> Result<KillCommandResponse, AgentError> {
        self.call(EXEC_KILL_PATH, request).await
    }

    pub async fn write_stdin(&self, request: &WriteStdinRequest) -> Result<WriteStdinResponse, AgentError> {
        self.call(EXEC_WRITE_STDIN_PATH, request).await
    }

    pub async fn write_files(&self, request: &WriteFilesRequest) -> Result<WriteFilesResponse, AgentError> {
        self.call(FILES_WRITE_PATH, request).await
    }

    pub async fn read_file(&self, request: &ReadFileRequest) -> Result<ReadFileResponse, AgentError> {
        self.call(FILES_READ_PATH, request).await
    }

    pub async fn create_user(
        &self,
        request: &CreateSandboxUserRequest,
    ) -> Result<CreateSandboxUserResponse, AgentError> {
        self.call(USERS_CREATE_PATH, request).await
    }

    pub async fn create_group(
        &self,
        request: &CreateSandboxGroupRequest,
    ) -> Result<CreateSandboxGroupResponse, AgentError> {
        self.call(GROUPS_CREATE_PATH, request).await
    }

    pub async fn modify_group_membership(
        &self,
        request: &ModifyGroupMembershipRequest,
    ) -> Result<ModifyGroupMembershipResponse, AgentError> {
        self.call(GROUP_MEMBERSHIP_PATH, request).await
    }

    async fn call<Req: Serialize, Res: DeserializeOwned>(&self, path: &str, request: &Req) -> Result<Res, AgentError> {
        self.within(self.timeouts.control, async {
            let response = self.send(path, request).await?;
            let response = self.check_status(response).await?;
            self.decode(response).await
        })
        .await
    }

    async fn within<T>(
        &self,
        timeout: Duration,
        work: impl Future<Output = Result<T, AgentError>>,
    ) -> Result<T, AgentError> {
        tokio::time::timeout(timeout, work).await.unwrap_or_else(|_| {
            Err(AgentError::Unresponsive {
                target: self.endpoint.to_string(),
                after: timeout,
            })
        })
    }

    async fn get(&self, path: &str) -> Result<Response<Incoming>, AgentError> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(hyper::header::HOST, HOST)
            .body(Full::new(Bytes::new()))
            .map_err(|e| self.unreachable(e.to_string()))?;
        self.send_once(request).await
    }

    async fn send<Req: Serialize>(&self, path: &str, request: &Req) -> Result<Response<Incoming>, AgentError> {
        let body = serde_json::to_vec(request).map_err(|e| self.unreachable(e.to_string()))?;
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(hyper::header::HOST, HOST)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| self.unreachable(e.to_string()))?;
        self.send_once(request).await
    }

    async fn check_status(&self, response: Response<Incoming>) -> Result<Response<Incoming>, AgentError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response
            .into_body()
            .collect()
            .await
            .map(|collected| collected.to_bytes())
            .unwrap_or_default();
        if status.as_u16() == PROTOCOL_REFUSED_STATUS
            && let Ok(reason) = serde_json::from_slice::<GuestProtocolError>(&body)
        {
            return Err(AgentError::ProtocolMismatch {
                target: self.endpoint.to_string(),
                reason,
            });
        }
        Err(AgentError::Http {
            target: self.endpoint.to_string(),
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).chars().take(512).collect(),
        })
    }

    async fn decode<T: DeserializeOwned>(&self, response: Response<Incoming>) -> Result<T, AgentError> {
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|e| self.unreachable(e.to_string()))?
            .to_bytes();
        serde_json::from_slice(&body).map_err(|e| AgentError::Malformed {
            target: self.endpoint.to_string(),
            message: e.to_string(),
        })
    }

    /// A frame we cannot read is dropped rather than fatal: losing one output chunk degrades
    /// the live view, while failing the command would throw away work that already ran.
    fn decode_frame(&self, line: &[u8]) -> Option<ExecFrame> {
        let line = trim_ascii_whitespace(line);
        if line.is_empty() {
            return None;
        }
        match serde_json::from_slice(line) {
            Ok(frame) => Some(frame),
            Err(e) => {
                warn!(target = %self.endpoint, error = %e, "dropping unreadable exec frame");
                None
            }
        }
    }

    fn unreachable(&self, message: String) -> AgentError {
        AgentError::Unreachable {
            target: self.endpoint.to_string(),
            message,
        }
    }
}

/// A command that runs to completion in `/` without streaming, as the worker issues for its own
/// guest-side housekeeping.
pub(crate) fn attached_command(lease_id: &LeaseId, command: Vec<String>, timeout_seconds: u32) -> RunCommandRequest {
    RunCommandRequest {
        lease_id: lease_id.clone(),
        command,
        cwd: Some("/".into()),
        env: CommandEnv::default(),
        timeout_seconds,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: OutputRetention::default(),
        max_output_bytes: None,
        redact_values: RedactValues::default(),
        launch_key: None,
        require_launch_cancellation: LaunchCancellation::NotRequired,
        execution: CommandExecution::Attached,
    }
}

/// The digest a guest reports for an entropy seed it applied.
fn seed_digest(seed: &[u8]) -> Result<trogon_sandbox_contract::Digest, trogon_sandbox_contract::DigestError> {
    trogon_sandbox_contract::Digest::new(format!("sha256:{}", hex::encode(sha2::Sha256::digest(seed))))
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &bytes[start..end]
}

impl GuestAgent {
    pub async fn verify_restore_clock_source(&self, lease_id: &LeaseId) -> Result<(), AgentError> {
        let request = attached_command(
            lease_id,
            vec![
                "/bin/cat".into(),
                "/sys/devices/system/clocksource/clocksource0/current_clocksource".into(),
            ],
            5,
        );
        match self.exec(&request, &DiscardChunks).await? {
            RunCommandResponse::Completed(output)
                if output.exit_code == 0 && output.stdout.trim_ascii() == b"kvm-clock" =>
            {
                Ok(())
            }
            _ => Err(AgentError::Malformed {
                target: self.endpoint.to_string(),
                message: "restored x86 guest must use the kvm-clock clock source".into(),
            }),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod protocol_tests;

#[cfg(test)]
mod test_support;
