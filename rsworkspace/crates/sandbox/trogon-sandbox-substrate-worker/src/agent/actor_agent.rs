use std::fmt::Display;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::warn;
use trogon_sandbox_actor_agent::{
    AGENT_PROTOCOL_VERSION_HEADER, ActorIdentity, BIND_PATH, BindLeaseRequest, EXEC_KILL_PATH, EXEC_PATH,
    EXEC_POLL_PATH, EXEC_WRITE_STDIN_PATH, ExecFrame, FILES_READ_PATH, FILES_WRITE_PATH, GROUP_MEMBERSHIP_PATH,
    GROUPS_CREATE_PATH, GUEST_PROTOCOL_VERSION, GuestProtocolError, IDENTITY_PATH, MIN_AGENT_PROTOCOL_VERSION_ENV,
    MIN_SUPPORTED_AGENT_PROTOCOL_VERSION, MinProtocolVersionError, PROTOCOL_REFUSED_STATUS, READYZ_PATH,
    USERS_CREATE_PATH, WORKER_PROTOCOL_VERSION_HEADER, admit_agent_version, try_min_protocol_version_from_env,
};
use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    KillCommandRequest, KillCommandResponse, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse,
    PollCommandRequest, PollCommandResponse, ReadFileRequest, ReadFileResponse, RunCommandRequest, RunCommandResponse,
    WriteFilesRequest, WriteFilesResponse, WriteStdinRequest, WriteStdinResponse,
};
use trogon_std::env::ReadEnv;

use super::actor_target::ActorTarget;
use super::agent_error::AgentError;
use super::chunk_sink::ChunkSink;
use super::constants::{
    CONNECT_AUTHORITY_HOST, CONTROL_TIMEOUT, ERROR_BODY_LIMIT, EXEC_TRANSPORT_MARGIN, READY_INITIAL_BACKOFF,
    READY_MAX_BACKOFF,
};
use super::read_connect_response::read_connect_response;
use crate::config::RouterEndpoint;
use crate::router_tls::RouterTlsConfig;

trait RouterStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RouterStream for T {}

/// A client for one actor's HTTP contract, reached through the router's CONNECT tunnel.
#[derive(Clone)]
pub struct ActorAgent {
    router: RouterEndpoint,
    target: ActorTarget,
    min_agent_protocol_version: u32,
    router_tls: Option<RouterTlsConfig>,
    control_timeout: Duration,
    exec_transport_margin: Duration,
}

impl ActorAgent {
    /// The oldest agent protocol this client drives comes from
    /// [`MIN_AGENT_PROTOCOL_VERSION_ENV`] in `env`, defaulting to the current version. An
    /// override that is set but is not an unsigned integer is refused.
    pub fn new(
        router: RouterEndpoint,
        target: ActorTarget,
        env: &impl ReadEnv,
    ) -> Result<Self, MinProtocolVersionError> {
        let minimum = try_min_protocol_version_from_env(
            env,
            MIN_AGENT_PROTOCOL_VERSION_ENV,
            MIN_SUPPORTED_AGENT_PROTOCOL_VERSION,
        )?;
        Ok(Self::with_minimum(router, target, minimum))
    }

    /// A client that drives agents at protocol `min_agent_protocol_version` or newer.
    pub fn with_minimum(router: RouterEndpoint, target: ActorTarget, min_agent_protocol_version: u32) -> Self {
        Self {
            router,
            target,
            router_tls: None,
            min_agent_protocol_version,
            control_timeout: CONTROL_TIMEOUT,
            exec_transport_margin: EXEC_TRANSPORT_MARGIN,
        }
    }

    pub fn with_router_tls(mut self, config: RouterTlsConfig) -> Self {
        self.router_tls = Some(config);
        self
    }

    /// Overrides the oldest agent protocol this client drives (zero admits agents that predate
    /// the handshake).
    pub fn with_min_agent_protocol_version(mut self, version: u32) -> Self {
        self.min_agent_protocol_version = version;
        self
    }

    /// Overrides how long a small control call may take, and how much wall-clock an exec may
    /// take beyond its own timeout, before the transport gives up.
    pub fn with_transport_timeouts(mut self, control: Duration, exec_margin: Duration) -> Self {
        self.control_timeout = control;
        self.exec_transport_margin = exec_margin;
        self
    }

    /// Blocks until the agent's HTTP server answers, or the deadline passes.
    ///
    /// `resume` already waited on the same probe server-side, so this is a defensive
    /// double-check, not the thing that confirms a lease is bound; `bind` still has to run
    /// after this returns.
    pub async fn wait_ready(&self, deadline: Duration) -> Result<(), AgentError> {
        let started = tokio::time::Instant::now();
        let mut backoff = READY_INITIAL_BACKOFF;
        loop {
            match self.get(READYZ_PATH, self.control_timeout).await {
                Ok(response) if response.status().is_success() => return Ok(()),
                Err(error @ AgentError::ProtocolMismatch { .. }) => return Err(error),
                _ => {}
            }

            if started.elapsed() >= deadline {
                return Err(AgentError::NotReady {
                    target: self.description(),
                    seconds: deadline.as_secs(),
                });
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(READY_MAX_BACKOFF);
        }
    }

    /// Tells a resumed actor which lease it now serves.
    ///
    /// Must be called before any exec: the actor is generic until it is bound, and the agent
    /// refuses commands for a lease it was not bound to.
    pub async fn bind(&self, request: &BindLeaseRequest) -> Result<(), AgentError> {
        let response = self.send(BIND_PATH, request, self.control_timeout).await?;
        self.check_status(response).await.map(|_| ())
    }

    pub async fn identity(&self) -> Result<ActorIdentity, AgentError> {
        let response = self.get(IDENTITY_PATH, self.control_timeout).await?;
        let response = self.check_status(response).await?;
        self.decode(response).await
    }

    /// Runs a command, forwarding every chunk to `sink` as it arrives.
    ///
    /// The body is newline-delimited and ends with exactly one response frame. A stream that
    /// ends without one is reported as malformed rather than silently treated as a success
    /// with empty output, because "the command produced nothing" and "the connection died
    /// mid-command" must not look the same to a caller deciding whether work happened.
    pub async fn exec(
        &self,
        request: &RunCommandRequest,
        sink: &dyn ChunkSink,
    ) -> Result<RunCommandResponse, AgentError> {
        let timeout = Duration::from_secs(u64::from(request.timeout_seconds)) + self.exec_transport_margin;
        let response = self.send(EXEC_PATH, request, timeout).await?;
        let response = self.check_status(response).await?;

        let mut body = response.into_body();
        let mut buffer = Vec::new();
        let mut outcome = None;

        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| self.unreachable_message(e.to_string()))?;
            // A frame that is not data (trailers) carries no output, so it adds nothing here.
            buffer.extend_from_slice(&frame.into_data().unwrap_or_default());
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=end).collect();
                self.dispatch_frame(&line, sink, &mut outcome).await;
            }
        }

        // A body without a trailing newline is still a complete frame.
        self.dispatch_frame(&buffer, sink, &mut outcome).await;

        outcome.ok_or_else(|| AgentError::Malformed {
            target: self.description(),
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

    /// Sandbox users are created inside the actor, because `ateapi.Control` has no exec RPC
    /// to run `useradd` through from out here.
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

    #[cfg(test)]
    pub(super) fn router(&self) -> &RouterEndpoint {
        &self.router
    }

    pub(super) async fn call<Req: Serialize, Res: DeserializeOwned>(
        &self,
        path: &str,
        request: &Req,
    ) -> Result<Res, AgentError> {
        let response = self.send(path, request, self.control_timeout).await?;
        let response = self.check_status(response).await?;
        self.decode(response).await
    }

    async fn get(&self, path: &str, timeout: Duration) -> Result<Response<Incoming>, AgentError> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(hyper::header::HOST, CONNECT_AUTHORITY_HOST)
            .body(Full::new(Bytes::new()))
            .map_err(|e| self.request_error(e))?;
        self.call_raw(request, timeout).await
    }

    async fn send<Req: Serialize>(
        &self,
        path: &str,
        request: &Req,
        timeout: Duration,
    ) -> Result<Response<Incoming>, AgentError> {
        let body = serde_json::to_vec(request).map_err(|e| self.request_error(e))?;
        let request = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(hyper::header::HOST, CONNECT_AUTHORITY_HOST)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| self.request_error(e))?;
        self.call_raw(request, timeout).await
    }

    /// Dials a fresh CONNECT tunnel and sends one request over it.
    ///
    /// Each call opens its own tunnel rather than sharing a persistent one across calls: the
    /// worker's control-plane traffic to one actor is low-volume, and a fresh tunnel per call
    /// avoids ever reusing a connection the router or the actor already decided to close.
    async fn call_raw(
        &self,
        request: Request<Full<Bytes>>,
        timeout: Duration,
    ) -> Result<Response<Incoming>, AgentError> {
        match tokio::time::timeout(timeout, self.send_once(request)).await {
            Ok(result) => result,
            Err(_) => Err(self.unreachable_because("request", format!("timed out after {}s", timeout.as_secs()))),
        }
    }

    async fn send_once(&self, mut request: Request<Full<Bytes>>) -> Result<Response<Incoming>, AgentError> {
        request.headers_mut().insert(
            WORKER_PROTOCOL_VERSION_HEADER,
            hyper::header::HeaderValue::from(GUEST_PROTOCOL_VERSION),
        );
        let mut sender = self.dial().await?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|e| self.unreachable_message(e.to_string()))?;
        self.admit_agent(&response)?;
        Ok(response)
    }

    /// Refuses an agent whose protocol version header is missing (version 0) or below the
    /// configured minimum, before any of its body is trusted.
    fn admit_agent(&self, response: &Response<Incoming>) -> Result<(), AgentError> {
        let header = response
            .headers()
            .get(AGENT_PROTOCOL_VERSION_HEADER)
            .and_then(|v| v.to_str().ok());
        admit_agent_version(header, self.min_agent_protocol_version)
            .map(|_| ())
            .map_err(|reason| AgentError::ProtocolMismatch {
                target: self.description(),
                reason,
            })
    }

    /// Opens a TCP connection to the router, tunnels it to [`Self::target`] with a CONNECT
    /// request carrying the `ate-target-actor` header, and hands back an HTTP/1.1 client
    /// bound to the tunneled connection.
    async fn dial(&self) -> Result<SendRequest<Full<Bytes>>, AgentError> {
        let mut stream = self.connect_router().await?;

        let connect_request = format!(
            "CONNECT {host}:{port} HTTP/1.1\r\n\
             Host: {host}:{port}\r\n\
             ate-target-actor: {header}\r\n\
             \r\n",
            host = CONNECT_AUTHORITY_HOST,
            port = self.target.port(),
            header = self.target.header_value(),
        );
        stream
            .write_all(connect_request.as_bytes())
            .await
            .map_err(|e| self.unreachable_because("sending CONNECT", e))?;
        stream
            .flush()
            .await
            .map_err(|e| self.unreachable_because("flushing CONNECT", e))?;

        read_connect_response(&mut stream)
            .await
            .map_err(|error| self.unreachable_message(error.to_string()))?;

        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| self.unreachable_because("HTTP handshake over tunnel", e))?;
        let description = self.description();
        tokio::spawn(async move { log_tunnel_end(&description, connection.await) });
        Ok(sender)
    }

    async fn connect_router(&self) -> Result<Box<dyn RouterStream>, AgentError> {
        let router_addr = format!("{}:{}", self.router.host(), self.router.port());
        match &self.router_tls {
            Some(tls) => {
                let stream = tls
                    .connect(&router_addr)
                    .await
                    .map_err(|e| self.unreachable_because("router TLS transport", e))?;
                Ok(Box::new(stream))
            }
            None => {
                let stream = TcpStream::connect(&router_addr)
                    .await
                    .map_err(|e| self.unreachable_because(&format!("connecting to router {router_addr}"), e))?;
                Ok(Box::new(stream))
            }
        }
    }

    pub(super) async fn check_status(&self, response: Response<Incoming>) -> Result<Response<Incoming>, AgentError> {
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
                target: self.description(),
                reason,
            });
        }
        Err(AgentError::Http {
            target: self.description(),
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).chars().take(ERROR_BODY_LIMIT).collect(),
        })
    }

    async fn decode<T: DeserializeOwned>(&self, response: Response<Incoming>) -> Result<T, AgentError> {
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|e| self.unreachable_message(e.to_string()))?
            .to_bytes();
        serde_json::from_slice(&body).map_err(|e| AgentError::Malformed {
            target: self.description(),
            message: e.to_string(),
        })
    }

    /// Routes one decoded line to the sink, or records the final response.
    async fn dispatch_frame(&self, line: &[u8], sink: &dyn ChunkSink, outcome: &mut Option<RunCommandResponse>) {
        match self.decode_frame(line) {
            Some(ExecFrame::Begin(begin)) => sink.begin(begin.begin_execution_id.into()).await,
            Some(ExecFrame::Chunk(chunk)) => sink.publish(chunk).await,
            Some(ExecFrame::Response(response)) => *outcome = Some(response),
            None => {}
        }
    }

    /// A frame we cannot read is dropped rather than fatal.
    ///
    /// Losing one output chunk degrades the live view; failing the whole command over it
    /// would throw away work that already ran and cannot be safely repeated.
    fn decode_frame(&self, line: &[u8]) -> Option<ExecFrame> {
        let line = line.trim_ascii();
        if line.is_empty() {
            return None;
        }
        match serde_json::from_slice(line) {
            Ok(frame) => Some(frame),
            Err(e) => {
                warn!(target = %self.description(), error = %e, "dropping unreadable exec frame");
                None
            }
        }
    }

    fn description(&self) -> String {
        format!("{} via {}", self.target, self.router)
    }

    fn request_error(&self, error: impl Display) -> AgentError {
        AgentError::Request {
            target: self.description(),
            message: error.to_string(),
        }
    }

    fn unreachable_message(&self, message: String) -> AgentError {
        AgentError::Unreachable {
            target: self.description(),
            message,
        }
    }

    fn unreachable_because(&self, context: &str, error: impl Display) -> AgentError {
        self.unreachable_message(format!("{context}: {error}"))
    }
}

/// Records why a tunnel connection ended abnormally.
pub(super) fn log_tunnel_end(description: &str, result: Result<(), impl Display>) {
    if let Err(error) = result {
        warn!(target = %description, %error, "actor agent tunnel connection ended");
    }
}
