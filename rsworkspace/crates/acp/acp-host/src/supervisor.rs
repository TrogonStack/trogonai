use crate::agent_launch::AgentLaunch;
use crate::initialized_agent::InitializedAgent;
use crate::restart_policy::RestartPolicy;
use acp_nats::boundary::{BoundaryExit, ConnectionAgent, connect_client_boundary_with};
use acp_nats::{AgentHandler, ClientHandler};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::InitializeRequest;
use agent_client_protocol::{AcpAgent, Client, ConnectTo};
use std::convert::Infallible;
use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{info, warn};

/// Produces the transport to a fresh agent. One operation, so tests can hand
/// the supervisor an in-process agent instead of a subprocess.
pub trait LaunchAgent {
    type Transport: ConnectTo<Client> + 'static;

    fn launch(&self) -> Self::Transport;
}

impl LaunchAgent for AgentLaunch {
    type Transport = AcpAgent;

    fn launch(&self) -> AcpAgent {
        self.to_agent()
    }
}

/// How one agent run ended.
#[derive(Debug)]
pub enum RunExit<E> {
    /// The agent closed its side: for a subprocess, it exited.
    AgentExited,
    /// The NATS side stopped serving without an error.
    ServeEnded,
    /// The NATS side failed; the agent is restarted with it.
    ServeFailed(E),
    /// The agent connection failed: the child could not be launched, failed
    /// its initialize handshake, or died abnormally (e.g. killed by a signal).
    AgentFailed(agent_client_protocol::Error),
}

impl<E: Display> Display for RunExit<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentExited => f.write_str("agent exited"),
            Self::ServeEnded => f.write_str("NATS serving ended"),
            Self::ServeFailed(error) => write!(f, "NATS serving failed: {error}"),
            Self::AgentFailed(error) => write!(f, "agent connection failed: {error}"),
        }
    }
}

/// Launches the agent, initializes it once, and hands it to `serve` until
/// either side ends.
pub async fn run_once<L, C, S, F, E>(launcher: &L, client: Arc<C>, serve: &S) -> RunExit<E>
where
    L: LaunchAgent,
    C: ClientHandler + Send + Sync + 'static,
    S: Fn(InitializedAgent<ConnectionAgent>) -> F,
    F: Future<Output = Result<(), E>>,
{
    let result = connect_client_boundary_with(client, launcher.launch(), async move |cx| {
        let agent = ConnectionAgent::new(cx);
        // Default client capabilities on purpose: the clients behind NATS are
        // chat bridges and the like, which serve no filesystem or terminal, so
        // the agent should use its own tools for both.
        let initialized = agent.initialize(InitializeRequest::new(ProtocolVersion::V1)).await?;
        info!(agent = ?initialized.agent_info, "agent initialized");
        Ok(serve(InitializedAgent::new(agent, initialized)).await)
    })
    .await;

    match result {
        Ok(BoundaryExit::TransportClosed) => RunExit::AgentExited,
        Ok(BoundaryExit::Main(Ok(()))) => RunExit::ServeEnded,
        Ok(BoundaryExit::Main(Err(error))) => RunExit::ServeFailed(error),
        Err(error) => RunExit::AgentFailed(error),
    }
}

/// Keeps one agent running behind `serve` for as long as the future is polled,
/// relaunching it per `policy` whenever a run ends. The caller stops it by
/// dropping the future (on shutdown), which also drops the child: the SDK
/// shuts a spawned agent down when its connection goes away.
///
/// Sessions do not survive a relaunch: the new process has never heard of
/// them, so a client's next call on an old session id fails and the client
/// opens a fresh one.
pub async fn supervise<L, C, S, F, E>(launcher: &L, client: Arc<C>, serve: S, policy: RestartPolicy) -> Infallible
where
    L: LaunchAgent,
    C: ClientHandler + Send + Sync + 'static,
    S: Fn(InitializedAgent<ConnectionAgent>) -> F,
    F: Future<Output = Result<(), E>>,
    E: Display,
    InitializedAgent<ConnectionAgent>: AgentHandler,
{
    let mut delay = None;
    loop {
        let started = Instant::now();
        let exit = run_once(launcher, client.clone(), &serve).await;
        let uptime = started.elapsed();
        let next = policy.next_delay(delay, uptime);
        warn!(%exit, uptime_ms = uptime.as_millis(), restart_in_ms = next.as_millis(), "agent run ended; relaunching");
        tokio::time::sleep(next).await;
        delay = Some(next);
    }
}

#[cfg(test)]
mod tests;
