use super::*;
use acp_nats::boundary::connect_agent_boundary;
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SessionNotification, StopReason,
};
use agent_client_protocol::{ByteStreams, Error, Result};
use async_compat::Compat;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

#[derive(Default)]
struct FakeAgent {
    initializes: AtomicU32,
}

#[async_trait::async_trait]
impl AgentHandler for FakeAgent {
    async fn initialize(
        &self,
        args: agent_client_protocol::v1::schema::InitializeRequest,
    ) -> Result<InitializeResponse> {
        self.initializes.fetch_add(1, Ordering::SeqCst);
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

struct NoopClient;

#[async_trait::async_trait]
impl ClientHandler for NoopClient {
    async fn request_permission(&self, _args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, _args: SessionNotification) -> Result<()> {
        Ok(())
    }
}

/// Launches a [`FakeAgent`] in-process for each run, on the far end of a pipe.
#[derive(Default)]
struct InProcessLauncher {
    agent: Arc<FakeAgent>,
}

impl LaunchAgent for InProcessLauncher {
    type Transport = ByteStreams<Compat<WriteHalf<DuplexStream>>, Compat<ReadHalf<DuplexStream>>>;

    fn launch(&self) -> Self::Transport {
        let (host_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (host_read, host_write) = tokio::io::split(host_io);
        let (agent_read, agent_write) = tokio::io::split(agent_io);
        tokio::spawn(connect_agent_boundary(
            self.agent.clone(),
            Compat::new(agent_write),
            Compat::new(agent_read),
            async move |_cx| std::future::pending::<Result<()>>().await,
        ));
        ByteStreams::new(Compat::new(host_write), Compat::new(host_read))
    }
}

/// An agent that exits the moment it starts.
#[derive(Default)]
struct ExitingLauncher {
    launches: AtomicU32,
}

impl LaunchAgent for ExitingLauncher {
    type Transport = ByteStreams<futures::io::Sink, futures::io::Empty>;

    fn launch(&self) -> Self::Transport {
        self.launches.fetch_add(1, Ordering::SeqCst);
        ByteStreams::new(futures::io::sink(), futures::io::empty())
    }
}

#[tokio::test]
async fn a_run_initializes_the_agent_once_and_serves_it_until_serving_ends() {
    let launcher = InProcessLauncher::default();
    let serve = async |agent: InitializedAgent<ConnectionAgent>| -> std::result::Result<(), Error> {
        // Two clients opening their own connections both initialize.
        agent
            .initialize(agent_client_protocol::v1::schema::InitializeRequest::new(
                ProtocolVersion::V1,
            ))
            .await?;
        agent
            .initialize(agent_client_protocol::v1::schema::InitializeRequest::new(
                ProtocolVersion::V1,
            ))
            .await?;
        let session = agent.new_session(NewSessionRequest::new("/tmp")).await?;
        let reply = agent.prompt(PromptRequest::new(session.session_id, vec![])).await?;
        assert_eq!(reply.stop_reason, StopReason::EndTurn);
        Ok(())
    };

    let exit = run_once(&launcher, Arc::new(NoopClient), &serve).await;

    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    assert_eq!(launcher.agent.initializes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_serving_failure_is_reported_as_such() {
    let launcher = InProcessLauncher::default();
    let serve = async |_agent: InitializedAgent<ConnectionAgent>| Err::<(), _>("nats down");

    let exit = run_once(&launcher, Arc::new(NoopClient), &serve).await;

    assert!(matches!(exit, RunExit::ServeFailed("nats down")), "got {exit}");
}

#[tokio::test]
async fn an_agent_that_exits_before_answering_ends_the_run() {
    let launcher = ExitingLauncher::default();
    let serve = async |_agent: InitializedAgent<ConnectionAgent>| Ok::<(), Error>(());

    let exit = run_once(&launcher, Arc::new(NoopClient), &serve).await;

    assert!(
        matches!(exit, RunExit::AgentExited | RunExit::AgentFailed(_)),
        "got {exit}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_agent_that_keeps_exiting_is_relaunched_with_growing_delays() {
    let launcher = ExitingLauncher::default();
    let serve = async |_agent: InitializedAgent<ConnectionAgent>| Ok::<(), Error>(());

    // Delays run 1s, 2s, 4s, 8s: four launches fit in 10s of paused time, a
    // fifth would need 15s.
    let supervised = supervise(&launcher, Arc::new(NoopClient), serve, RestartPolicy::DEFAULT);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), supervised).await;

    assert_eq!(launcher.launches.load(Ordering::SeqCst), 4);
}
