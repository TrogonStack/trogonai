use crate::agent_launch::AgentLaunch;
use crate::client_claim::ClientClaim;
use crate::client_handshake::{ClientHandshake, ClientInitialize, Handshake};
use crate::constants::INITIALIZE_TIMEOUT;
use crate::initialized_agent::InitializedAgent;
use crate::restart_policy::RestartPolicy;
use acp_nats::boundary::{BoundaryExit, ConnectionAgent, connect_client_boundary_with};
use acp_nats::{AgentHandler, ClientHandler};
use agent_client_protocol::v1::schema::InitializeResponse;
use agent_client_protocol::{AcpAgent, Client, ConnectTo};
use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
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
    /// The agent connection failed: the child could not be launched, refused
    /// the client's initialize it was relaunched with or did not answer it in
    /// time, or died abnormally (e.g. killed by a signal).
    AgentFailed(agent_client_protocol::Error),
    /// The client initialized again with different parameters, which the
    /// running child cannot take: the next child is initialized with them.
    Reinitialize,
}

impl<E: Display> Display for RunExit<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentExited => f.write_str("agent exited"),
            Self::ServeEnded => f.write_str("NATS serving ended"),
            Self::ServeFailed(error) => write!(f, "NATS serving failed: {error}"),
            Self::AgentFailed(error) => write!(f, "agent connection failed: {error}"),
            Self::Reinitialize => f.write_str("the client initialized with different parameters"),
        }
    }
}

/// How a run's serving ended, short of the agent going away.
enum Served<E> {
    Ended(Result<(), E>),
    Reinitialize,
}

/// Launches the agent and hands it to `serve` until either side ends.
///
/// When `handshake` holds a client's `initialize` (this is a relaunch), the
/// child is initialized with it before serving starts, and has
/// [`INITIALIZE_TIMEOUT`] to answer before the run fails. Otherwise the child is
/// left for the first client `initialize` to reach it; see
/// [`InitializedAgent`].
pub async fn run_once<L, C, S, F, E>(
    launcher: &L,
    client: Arc<C>,
    claim: Arc<ClientClaim>,
    handshake: Arc<ClientHandshake>,
    serve: &S,
) -> RunExit<E>
where
    L: LaunchAgent,
    C: ClientHandler + Send + Sync + 'static,
    S: Fn(InitializedAgent<ConnectionAgent>) -> F,
    F: Future<Output = Result<(), E>>,
{
    // Set once this run chose to make way for a relaunch. The SDK shuts the
    // child down after that, and a child that exits abnormally on the way out
    // turns the run's result into an error; the run still ended because the
    // client asked for a relaunch, not because the child failed, so it must
    // not wait out the crash backoff with nothing serving the prefix.
    let reinitialize = Arc::new(AtomicBool::new(false));
    let reinitializing = reinitialize.clone();
    let result = connect_client_boundary_with(client, launcher.launch(), async move |cx| {
        let agent = ConnectionAgent::new(cx);
        let initialized = match handshake.next() {
            Some(request) => {
                let response = initialize_relaunched(&agent, &handshake, &request).await?;
                info!(agent = ?response.agent_info, "agent relaunched with the client's initialize");
                Some(Handshake { request, response })
            }
            None => None,
        };
        let agent = InitializedAgent::new(agent, initialized, claim, handshake.clone());
        Ok(tokio::select! {
            result = serve(agent) => Served::Ended(result),
            () = handshake.relaunch_requested() => {
                reinitializing.store(true, Ordering::SeqCst);
                Served::Reinitialize
            }
        })
    })
    .await;

    match result {
        Ok(BoundaryExit::Main(Served::Reinitialize)) => RunExit::Reinitialize,
        _ if reinitialize.load(Ordering::SeqCst) => RunExit::Reinitialize,
        Ok(BoundaryExit::TransportClosed) => RunExit::AgentExited,
        Ok(BoundaryExit::Main(Served::Ended(Ok(())))) => RunExit::ServeEnded,
        Ok(BoundaryExit::Main(Served::Ended(Err(error)))) => RunExit::ServeFailed(error),
        Err(error) => RunExit::AgentFailed(error),
    }
}

/// Initializes a relaunched child with the client's `request`, giving it
/// [`INITIALIZE_TIMEOUT`] to answer, and tells `handshake` how that went.
async fn initialize_relaunched(
    agent: &ConnectionAgent,
    handshake: &ClientHandshake,
    request: &ClientInitialize,
) -> agent_client_protocol::Result<InitializeResponse> {
    let Ok(result) = tokio::time::timeout(INITIALIZE_TIMEOUT, agent.initialize_raw(request.params().clone())).await
    else {
        return Err(handshake.time_out(request));
    };
    handshake.settle(request, &result);
    result
}

/// Keeps one agent running behind `serve`, relaunching it per `policy`
/// whenever a run ends, until `shutdown` is cancelled. `serve` is expected to
/// end its run on the same signal (see [`crate::serving::serve`]); the run
/// that ends then is not relaunched, and this returns. Its child goes with it:
/// the SDK shuts a spawned agent down when its connection goes away.
///
/// The one [`ClientClaim`] spans every run, so the client a host serves stays
/// the same across relaunches, and so does the client's `initialize`: every
/// child after the first is initialized with the claiming client's last one.
/// A relaunch the client asked for by initializing with different parameters
/// happens once the client's burst of them is over (see
/// [`ClientHandshake::relaunch_requested`]), not after the restart delay, and does not count
/// toward the backoff.
///
/// Whether a session survives a relaunch depends on the agent. One that keeps
/// sessions on disk (fx does) reloads it when the client sends `session/load`;
/// one that keeps them only in memory has never heard of the old id, so the
/// client's next call on it fails and the client opens a fresh session.
pub async fn supervise<L, C, S, F, E>(
    launcher: &L,
    client: Arc<C>,
    claim: Arc<ClientClaim>,
    serve: S,
    policy: RestartPolicy,
    shutdown: &CancellationToken,
) where
    L: LaunchAgent,
    C: ClientHandler + Send + Sync + 'static,
    S: Fn(InitializedAgent<ConnectionAgent>) -> F,
    F: Future<Output = Result<(), E>>,
    E: Display,
{
    let handshake = Arc::new(ClientHandshake::default());
    let mut delay = None;
    while !shutdown.is_cancelled() {
        let started = Instant::now();
        let exit = run_once(launcher, client.clone(), claim.clone(), handshake.clone(), &serve).await;
        let uptime = started.elapsed();
        if shutdown.is_cancelled() {
            info!(%exit, uptime_ms = uptime.as_millis(), "agent run ended for shutdown");
            return;
        }
        if matches!(exit, RunExit::Reinitialize) {
            info!(%exit, uptime_ms = uptime.as_millis(), "relaunching the agent");
            delay = None;
            continue;
        }
        let next = policy.next_delay(delay, uptime);
        warn!(%exit, uptime_ms = uptime.as_millis(), restart_in_ms = next.as_millis(), "agent run ended; relaunching");
        tokio::select! {
            () = tokio::time::sleep(next) => {}
            () = shutdown.cancelled() => return,
        }
        delay = Some(next);
    }
}

#[cfg(test)]
mod tests;
