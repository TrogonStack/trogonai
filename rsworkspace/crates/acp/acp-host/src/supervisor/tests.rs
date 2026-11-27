use super::*;
use acp_nats::boundary::connect_agent_boundary;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, ClientCapabilities, Implementation,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionNotification, StopReason,
};
use agent_client_protocol::{ByteStreams, Error, Result};
use async_compat::Compat;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

/// The child: it records every `initialize` exactly as it arrives over the
/// pipe, so a test sees what a real child process would.
#[derive(Default)]
struct FakeAgent {
    initializes: std::sync::Mutex<Vec<InitializeRequest>>,
}

impl FakeAgent {
    fn initializes(&self) -> Vec<InitializeRequest> {
        self.initializes.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AgentHandler for FakeAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        let reply = InitializeResponse::new(args.protocol_version)
            .agent_info(Implementation::new("fake-agent", self.initializes().len().to_string()));
        self.initializes.lock().unwrap().push(args);
        Ok(reply)
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

/// One run of `serve` on a child from `launcher`, for a test that neither
/// shuts the host down nor looks at the client side.
async fn run<L, S, F, E>(
    launcher: &L,
    claim: Arc<ClientClaim>,
    handshake: Arc<ClientHandshake>,
    serve: &S,
) -> RunExit<E>
where
    L: LaunchAgent,
    S: Fn(InitializedAgent<ConnectionAgent>, RunStop) -> F,
    F: Future<Output = std::result::Result<(), E>>,
{
    run_once(
        launcher,
        Arc::new(NoopClient),
        claim,
        handshake,
        serve,
        &CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn a_client_that_initializes_twice_alike_initializes_the_child_once() {
    let launcher = InProcessLauncher::default();
    let serve = async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
        // The client initializes again, as it does after it restarts.
        agent.initialize(InitializeRequest::new(ProtocolVersion::V1)).await?;
        agent.initialize(InitializeRequest::new(ProtocolVersion::V1)).await?;
        let session = agent.new_session(NewSessionRequest::new("/tmp")).await?;
        let reply = agent.prompt(PromptRequest::new(session.session_id, vec![])).await?;
        assert_eq!(reply.stop_reason, StopReason::EndTurn);
        Ok(())
    };

    let exit = run(&launcher, Arc::default(), Arc::default(), &serve).await;

    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    assert_eq!(launcher.agent.initializes().len(), 1);
}

#[tokio::test]
async fn a_second_client_is_refused_and_the_first_keeps_working() {
    let launcher = InProcessLauncher::default();
    let serve = async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
        let first = InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new("discord", "1.0.0"));
        let second = InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new("zed", "1.0.0"));
        agent.initialize(first.clone()).await?;
        let session = agent.new_session(NewSessionRequest::new("/tmp")).await?;

        let refusal = agent.initialize(second).await.expect_err("second client refused");
        assert_eq!(refusal.code, agent_client_protocol::ErrorCode::INVALID_REQUEST);
        assert!(refusal.message.contains("serves one client"), "{}", refusal.message);

        agent.initialize(first).await?;
        let reply = agent.prompt(PromptRequest::new(session.session_id, vec![])).await?;
        assert_eq!(reply.stop_reason, StopReason::EndTurn);
        Ok(())
    };

    let exit = run(&launcher, Arc::default(), Arc::default(), &serve).await;

    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
}

#[tokio::test]
async fn the_claim_outlives_a_relaunch() {
    let launcher = InProcessLauncher::default();
    let claim = Arc::new(ClientClaim::default());
    let first = async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
        agent
            .initialize(InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new("discord", "1")))
            .await?;
        Ok(())
    };
    let second = async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
        agent
            .initialize(InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new("zed", "1")))
            .await
            .map(|_| ())
    };

    let exit = run(&launcher, claim.clone(), Arc::default(), &first).await;
    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    let exit = run(&launcher, claim, Arc::default(), &second).await;

    assert!(
        matches!(exit, RunExit::ServeFailed(ref e) if e.code == agent_client_protocol::ErrorCode::INVALID_REQUEST),
        "got {exit}"
    );
}

#[tokio::test]
async fn a_serving_failure_is_reported_as_such() {
    let launcher = InProcessLauncher::default();
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| Err::<(), _>("nats down");

    let exit = run(&launcher, Arc::default(), Arc::default(), &serve).await;

    assert!(matches!(exit, RunExit::ServeFailed("nats down")), "got {exit}");
}

#[tokio::test]
async fn an_agent_that_exits_before_answering_ends_the_run() {
    let launcher = ExitingLauncher::default();
    // Serves until the agent goes away, as the NATS side does.
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, stop: RunStop| {
        stop.await;
        Ok::<(), Error>(())
    };

    let exit = run(&launcher, Arc::default(), Arc::default(), &serve).await;

    assert!(
        matches!(exit, RunExit::AgentExited | RunExit::AgentFailed(_)),
        "got {exit}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_agent_that_keeps_exiting_is_relaunched_with_growing_delays() {
    let launcher = ExitingLauncher::default();
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, stop: RunStop| {
        stop.await;
        Ok::<(), Error>(())
    };

    // Delays run 1s, 2s, 4s, 8s: four launches fit in 10s of paused time, a
    // fifth would need 15s.
    let shutdown = CancellationToken::new();
    let supervised = supervise(
        &launcher,
        Arc::new(NoopClient),
        Arc::default(),
        serve,
        RestartPolicy::DEFAULT,
        &shutdown,
    );
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), supervised).await;

    assert_eq!(launcher.launches.load(Ordering::SeqCst), 4);
}

fn meta(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        _ => panic!("meta must be an object"),
    }
}

/// An fx-style client: host tools offered in the capabilities' `_meta`, and
/// `_meta` on the request itself.
fn discord(version: &str) -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(ClientCapabilities::new().terminal(true).meta(meta(
            serde_json::json!({"libtrogonai.tools": [{"name": "send_message"}]}),
        )))
        .client_info(Implementation::new("discord", version))
        .meta(meta(serde_json::json!({"traceparent": "00-abc-def-01"})))
}

#[tokio::test]
async fn the_child_receives_the_clients_initialize_as_sent() {
    let launcher = InProcessLauncher::default();
    let serve = async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
        let answer = agent.initialize(discord("1")).await?;
        assert_eq!(
            answer.agent_info,
            Some(Implementation::new("fake-agent", "0")),
            "the child's own answer"
        );
        Ok(())
    };

    let exit = run(&launcher, Arc::default(), Arc::default(), &serve).await;

    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    assert_eq!(launcher.agent.initializes(), vec![discord("1")]);
}

#[tokio::test]
async fn a_relaunched_child_is_initialized_with_the_clients_last_initialize() {
    let launcher = InProcessLauncher::default();
    let handshake = Arc::new(ClientHandshake::default());
    let claim = Arc::new(ClientClaim::default());
    let first_run =
        async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
            agent.initialize(discord("1")).await?;
            Ok(())
        };
    // The client does not initialize again: the host's child crashed, not the client.
    let second_run =
        async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
            let session = agent.new_session(NewSessionRequest::new("/tmp")).await?;
            agent.prompt(PromptRequest::new(session.session_id, vec![])).await?;
            Ok(())
        };

    let exit = run(&launcher, claim.clone(), handshake.clone(), &first_run).await;
    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    let exit = run(&launcher, claim, handshake, &second_run).await;

    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    assert_eq!(launcher.agent.initializes(), vec![discord("1"), discord("1")]);
}

#[tokio::test]
async fn a_different_initialize_relaunches_the_child_with_it() {
    let launcher = InProcessLauncher::default();
    let handshake = Arc::new(ClientHandshake::default());
    let claim = Arc::new(ClientClaim::default());
    let (answered_tx, answered_rx) = std::sync::mpsc::channel();
    let first_run = async |agent: InitializedAgent<ConnectionAgent>, stop: RunStop| -> std::result::Result<(), Error> {
        let agent = Arc::new(agent);
        agent.initialize(discord("1")).await?;
        // The client restarted with a new version. Its initialize runs as a
        // task of its own, as acp-nats-agent dispatches it, and outlives the run.
        let reinitialize = agent.clone();
        let answered = answered_tx.clone();
        tokio::spawn(async move {
            let _ = answered.send(reinitialize.initialize(discord("2")).await);
        });
        stop.await;
        Ok(())
    };
    let second_run =
        async |_agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> { Ok(()) };

    let exit = run(&launcher, claim.clone(), handshake.clone(), &first_run).await;
    assert!(matches!(exit, RunExit::Reinitialize), "got {exit}");
    let exit = run(&launcher, claim, handshake, &second_run).await;
    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");

    assert_eq!(launcher.agent.initializes(), vec![discord("1"), discord("2")]);
    let answer = tokio::task::spawn_blocking(move || answered_rx.recv().unwrap())
        .await
        .unwrap()
        .expect("answered");
    assert_eq!(
        answer.agent_info,
        Some(Implementation::new("fake-agent", "1")),
        "the new child's answer"
    );
}

#[tokio::test]
async fn before_any_client_initializes_the_child_gets_defaults_on_its_first_request() {
    let launcher = InProcessLauncher::default();
    let serve = async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| -> std::result::Result<(), Error> {
        agent.new_session(NewSessionRequest::new("/tmp")).await?;
        Ok(())
    };

    let exit = run(&launcher, Arc::default(), Arc::default(), &serve).await;

    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    assert_eq!(
        launcher.agent.initializes(),
        vec![InitializeRequest::new(ProtocolVersion::V1)]
    );
}

#[tokio::test]
async fn a_launched_child_waits_for_the_client_instead_of_initializing_itself() {
    let launcher = InProcessLauncher::default();
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| Ok::<(), Error>(());

    let exit = run(&launcher, Arc::default(), Arc::default(), &serve).await;

    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    assert!(launcher.agent.initializes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_relaunch_the_client_asks_for_skips_the_restart_delay() {
    let launcher = InProcessLauncher::default();
    let runs = AtomicU32::new(0);
    let serve = async |agent: InitializedAgent<ConnectionAgent>, stop: RunStop| -> std::result::Result<(), Error> {
        if runs.fetch_add(1, Ordering::SeqCst) == 0 {
            let agent = Arc::new(agent);
            agent.initialize(discord("1")).await?;
            let reinitialize = agent.clone();
            tokio::spawn(async move { reinitialize.initialize(discord("2")).await });
        }
        stop.await;
        Ok(())
    };

    // A crash relaunch would wait a second first.
    let shutdown = CancellationToken::new();
    let supervised = supervise(
        &launcher,
        Arc::new(NoopClient),
        Arc::default(),
        serve,
        RestartPolicy::DEFAULT,
        &shutdown,
    );
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), supervised).await;

    assert_eq!(launcher.agent.initializes(), vec![discord("1"), discord("2")]);
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_the_restart_delay_ends_supervision_without_another_launch() {
    let launcher = ExitingLauncher::default();
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, stop: RunStop| {
        stop.await;
        Ok::<(), Error>(())
    };
    let shutdown = CancellationToken::new();
    let supervised = supervise(
        &launcher,
        Arc::new(NoopClient),
        Arc::default(),
        serve,
        RestartPolicy::DEFAULT,
        &shutdown,
    );
    tokio::pin!(supervised);

    // The first run exits at once; the next launch is a second away.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut supervised).await;
    shutdown.cancel();

    tokio::time::timeout(std::time::Duration::from_millis(1), supervised)
        .await
        .expect("supervision returns as soon as shutdown is asked for");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
}

/// A child that records every JSON-RPC message it reads as raw JSON, so a test
/// sees members no typed request has, and answers `initialize`.
#[derive(Default)]
struct RawLauncher {
    seen: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl RawLauncher {
    fn initialize_params(&self) -> Vec<serde_json::Value> {
        let seen = self.seen.lock().unwrap();
        seen.iter()
            .filter(|message| message["method"] == "initialize")
            .map(|message| message["params"].clone())
            .collect()
    }
}

impl LaunchAgent for RawLauncher {
    type Transport = ByteStreams<Compat<WriteHalf<DuplexStream>>, Compat<ReadHalf<DuplexStream>>>;

    fn launch(&self) -> Self::Transport {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let (host_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (host_read, host_write) = tokio::io::split(host_io);
        let (agent_read, mut agent_write) = tokio::io::split(agent_io);
        let seen = self.seen.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(agent_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let message: serde_json::Value = serde_json::from_str(&line).unwrap();
                seen.lock().unwrap().push(message.clone());
                if message["method"] == "initialize" {
                    let reply = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {"protocolVersion": message["params"]["protocolVersion"]},
                    });
                    agent_write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                }
            }
        });
        ByteStreams::new(Compat::new(host_write), Compat::new(host_read))
    }
}

#[tokio::test]
async fn capabilities_the_schema_does_not_name_reach_every_child() {
    let launcher = RawLauncher::default();
    let handshake = Arc::new(ClientHandshake::default());
    let sent = serde_json::json!({
        "protocolVersion": 1,
        "clientCapabilities": {
            "terminal": true,
            "libtrogonai": {"tools": [{"name": "send_message"}]},
        },
        "clientInfo": {"name": "discord", "version": "1"},
    });
    let first = async |agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| {
        agent.initialize_raw(sent.clone()).await.map(drop)
    };
    let relaunched = async |_agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| Ok::<(), Error>(());

    let exit = run(&launcher, Arc::default(), handshake.clone(), &first).await;
    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");
    // A relaunch (after a crash, say) initializes the next child with the
    // same params.
    let exit = run(&launcher, Arc::default(), handshake, &relaunched).await;
    assert!(matches!(exit, RunExit::ServeEnded), "got {exit}");

    assert_eq!(launcher.initialize_params(), vec![sent.clone(), sent]);
}

/// A child that keeps its pipe open and never answers anything.
struct SilentLauncher;

impl LaunchAgent for SilentLauncher {
    type Transport = ByteStreams<Compat<WriteHalf<DuplexStream>>, Compat<ReadHalf<DuplexStream>>>;

    fn launch(&self) -> Self::Transport {
        let (host_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (host_read, host_write) = tokio::io::split(host_io);
        tokio::spawn(async move {
            let (mut agent_read, _agent_write) = tokio::io::split(agent_io);
            let _ = tokio::io::copy(&mut agent_read, &mut tokio::io::sink()).await;
        });
        ByteStreams::new(Compat::new(host_write), Compat::new(host_read))
    }
}

#[tokio::test(start_paused = true)]
async fn a_relaunched_child_that_never_answers_initialize_fails_the_run_and_keeps_the_handshake() {
    let handshake = Arc::new(ClientHandshake::default());
    // The client asked for a relaunch with it.
    let _pending = handshake.relaunch_with(discord("1").into());
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| Ok::<(), Error>(());

    let exit = run(&SilentLauncher, Arc::default(), handshake.clone(), &serve).await;

    assert!(
        matches!(&exit, RunExit::AgentFailed(_)) && exit.to_string().contains("did not answer initialize"),
        "got {exit}"
    );
    assert_eq!(
        handshake.next(),
        Some(discord("1").into()),
        "the next child gets it too"
    );
}

#[tokio::test]
async fn a_relaunched_child_that_exits_before_answering_keeps_the_handshake() {
    let handshake = Arc::new(ClientHandshake::default());
    // The client asked for a relaunch with it.
    let _pending = handshake.relaunch_with(discord("1").into());
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, _stop: RunStop| Ok::<(), Error>(());

    let exit = run(&ExitingLauncher::default(), Arc::default(), handshake.clone(), &serve).await;

    assert!(
        matches!(exit, RunExit::AgentExited | RunExit::AgentFailed(_)),
        "got {exit}"
    );
    assert_eq!(handshake.next(), Some(discord("1").into()));
}

#[tokio::test]
async fn concurrent_different_initializes_are_all_answered_by_one_relaunch() {
    let launcher = InProcessLauncher::default();
    let (answers_tx, mut answers) = tokio::sync::mpsc::unbounded_channel();
    let runs = AtomicU32::new(0);
    let serve = async |agent: InitializedAgent<ConnectionAgent>, stop: RunStop| -> std::result::Result<(), Error> {
        if runs.fetch_add(1, Ordering::SeqCst) == 0 {
            let agent = Arc::new(agent);
            agent.initialize(discord("0")).await?;
            // Ten initializes from the client at once, each with params of its own.
            for i in 1..=10 {
                let agent = agent.clone();
                let answers = answers_tx.clone();
                tokio::spawn(async move {
                    let _ = answers.send(agent.initialize(discord(&i.to_string())).await);
                });
            }
        }
        stop.await;
        Ok(())
    };
    let shutdown = CancellationToken::new();
    let supervised = supervise(
        &launcher,
        Arc::new(NoopClient),
        Arc::default(),
        serve,
        RestartPolicy::DEFAULT,
        &shutdown,
    );

    let collected = async {
        let mut results = Vec::new();
        while results.len() < 10 {
            results.push(answers.recv().await.unwrap());
        }
        results
    };
    let results = tokio::select! {
        results = collected => results,
        () = supervised => panic!("supervision ended"),
        () = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("an initialize hung"),
    };

    // All ten reach the host before the running child is replaced, so the
    // newest is the one relaunch and the other nine are superseded.
    let answered = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(answered, 1, "{results:?}");
    for error in results.iter().filter_map(|result| result.as_ref().err()) {
        assert!(
            error.message.contains("newer one"),
            "only superseded ones fail: {error:?}"
        );
    }
    assert_eq!(
        launcher.agent.initializes(),
        vec![discord("0"), discord("10")],
        "one relaunch, with the newest params"
    );
}

#[tokio::test]
async fn a_request_naming_a_client_before_any_initialize_does_not_lock_out_the_real_client() {
    let launcher = InProcessLauncher::default();
    let claim = Arc::new(ClientClaim::default());
    let (answer_tx, answer) = tokio::sync::oneshot::channel();
    let answer_tx = std::sync::Mutex::new(Some(answer_tx));
    let runs = AtomicU32::new(0);
    let serve = async |agent: InitializedAgent<ConnectionAgent>, stop: RunStop| -> std::result::Result<(), Error> {
        if runs.fetch_add(1, Ordering::SeqCst) == 0 {
            // A stray client's request comes first. The gate lets it through
            // without claiming, and it initializes the child with defaults.
            acp_nats_agent::ClientGate::admit(agent.claim().as_ref(), Some("squatter"))?;
            agent.new_session(NewSessionRequest::new("/tmp")).await?;
            // Then the real client initializes, relaunching the child.
            let agent = Arc::new(agent);
            let answer_tx = answer_tx.lock().unwrap().take().unwrap();
            tokio::spawn(async move {
                let _ = answer_tx.send(agent.initialize(discord("1")).await);
            });
        }
        stop.await;
        Ok(())
    };
    let shutdown = CancellationToken::new();
    let supervised = supervise(
        &launcher,
        Arc::new(NoopClient),
        claim.clone(),
        serve,
        RestartPolicy::DEFAULT,
        &shutdown,
    );

    let answer = tokio::select! {
        answer = answer => answer.unwrap(),
        () = supervised => panic!("supervision ended"),
        () = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("the real client's initialize hung"),
    };

    answer.expect("the real client initializes");
    assert_eq!(
        launcher.agent.initializes(),
        vec![InitializeRequest::new(ProtocolVersion::V1), discord("1")]
    );
    acp_nats_agent::ClientGate::admit(claim.as_ref(), Some("squatter")).expect_err("the stray client is refused");
}

#[tokio::test(start_paused = true)]
async fn a_serving_failure_ends_the_run_and_serving_starts_again() {
    let launcher = InProcessLauncher::default();
    let serves = AtomicU32::new(0);
    // The first run's serving fails, as it does when NATS lost the COMMANDS
    // stream; the next run's serving (which provisions again) holds.
    let serve = async |_agent: InitializedAgent<ConnectionAgent>, stop: RunStop| -> std::result::Result<(), &str> {
        if serves.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err("commands stream missing");
        }
        stop.await;
        Ok(())
    };
    let shutdown = CancellationToken::new();
    let supervised = supervise(
        &launcher,
        Arc::new(NoopClient),
        Arc::default(),
        serve,
        RestartPolicy::DEFAULT,
        &shutdown,
    );

    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), supervised).await;

    assert_eq!(serves.load(Ordering::SeqCst), 2);
}

mod redelivery;
