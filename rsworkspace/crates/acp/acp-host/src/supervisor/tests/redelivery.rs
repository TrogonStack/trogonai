//! A command in flight when a run ends for a relaunch or because the child
//! went away is handed back for redelivery at once, not left to wait out
//! `ack_wait` (see [`run_once`]).

use super::*;
use crate::serving;
use acp_nats::AcpPrefix;
use acp_nats::jetstream::streams::commands_stream_name;
use acp_nats::wire::encode_request;
use agent_client_protocol::v1::schema::{LoadSessionRequest, LoadSessionResponse};
use async_nats::jetstream::consumer::{PullConsumer, pull};
use async_nats::jetstream::stream::Stream;
use futures::StreamExt;
use jsonrpc_nats::RequestId;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_nats::test_support::JetStreamTestServer;

/// Well under the consumer's 30 s `ack_wait`: a command that is only
/// redelivered after `ack_wait` cannot arrive inside it.
const PROMPTLY: Duration = Duration::from_secs(5);

/// A child whose `session/load` never finishes: the command it is working on
/// when the run ends.
#[derive(Default)]
struct StuckLoad {
    started: Notify,
}

#[async_trait::async_trait]
impl AgentHandler for StuckLoad {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("s1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }

    async fn load_session(&self, _args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        self.started.notify_one();
        std::future::pending().await
    }
}

/// Launches a [`StuckLoad`] in-process, keeping its task so a test can kill
/// the child.
#[derive(Default)]
struct StuckLauncher {
    agent: Arc<StuckLoad>,
    child: std::sync::Mutex<Option<JoinHandle<Result<acp_nats::boundary::BoundaryExit<()>>>>>,
}

impl StuckLauncher {
    fn kill(&self) {
        self.child.lock().unwrap().take().expect("launched").abort();
    }
}

impl LaunchAgent for StuckLauncher {
    type Transport = ByteStreams<Compat<WriteHalf<DuplexStream>>, Compat<ReadHalf<DuplexStream>>>;

    fn launch(&self) -> Self::Transport {
        let (host_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (host_read, host_write) = tokio::io::split(host_io);
        let (agent_read, agent_write) = tokio::io::split(agent_io);
        let child = tokio::spawn(connect_agent_boundary(
            self.agent.clone(),
            Compat::new(agent_write),
            Compat::new(agent_read),
            async move |_cx| std::future::pending::<Result<()>>().await,
        ));
        *self.child.lock().unwrap() = Some(child);
        ByteStreams::new(Compat::new(host_write), Compat::new(host_read))
    }
}

/// Waits until serving has created its consumer on COMMANDS, and names it. A
/// consumer that only delivers new commands would skip one published before it
/// exists.
async fn commands_consumer(stream: &mut Stream) -> String {
    tokio::time::timeout(PROMPTLY, async {
        while stream.info().await.unwrap().state.consumer_count == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("serving creates its COMMANDS consumer");
    stream.consumer_names().next().await.unwrap().unwrap()
}

/// Runs one child behind real serving, gets a `session/load` in flight on it,
/// ends the run with `end`, and checks the run's consumer hands the command
/// out again before `ack_wait`.
async fn assert_redelivered_when_the_run_ends(
    prefix: &str,
    end: impl FnOnce(&StuckLauncher, &ClientHandshake),
    expected: impl Fn(&RunExit<acp_nats_agent::ConnectionError>) -> bool,
) {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let jetstream = async_nats::jetstream::new(nats.clone());
    let js = NatsJetStreamClient::new(jetstream.clone());
    let prefix = AcpPrefix::new(prefix).unwrap();
    crate::streams::provision(&js, &prefix).await.expect("provision");
    let mut stream = jetstream.get_stream(commands_stream_name(&prefix)).await.unwrap();

    let launcher = StuckLauncher::default();
    let handshake = Arc::new(ClientHandshake::default());
    let serve = |agent, stop| serving::serve(agent, nats.clone(), js.clone(), prefix.clone(), stop);
    let shutdown = CancellationToken::new();

    let local = tokio::task::LocalSet::new();
    let (exit, consumer) = local
        .run_until(async {
            let run = run_once(
                &launcher,
                Arc::new(NoopClient),
                Arc::default(),
                handshake.clone(),
                &serve,
                &shutdown,
            );
            tokio::pin!(run);
            let consumer = tokio::select! {
                exit = &mut run => panic!("the run ended early: {exit}"),
                consumer = async {
                    let consumer = commands_consumer(&mut stream).await;
                    let command = encode_request(
                        "session/load",
                        RequestId::String("load-1".into()),
                        &LoadSessionRequest::new("s1", "/tmp"),
                    )
                    .unwrap();
                    let subject = format!("{prefix}.v1.session.s1.agent.load", prefix = prefix.as_str());
                    jetstream
                        .publish_with_headers(subject, command.headers, command.body)
                        .await
                        .unwrap()
                        .await
                        .expect("COMMANDS stores the command");
                    tokio::time::timeout(PROMPTLY, launcher.agent.started.notified())
                        .await
                        .expect("the child starts on the command");
                    consumer
                } => consumer,
            };
            end(&launcher, &handshake);
            let exit = tokio::time::timeout(PROMPTLY, run).await.expect("the run ends");
            (exit, consumer)
        })
        .await;
    assert!(expected(&exit), "got {exit}");
    nats.flush().await.unwrap();

    let consumer: PullConsumer = stream.get_consumer::<pull::Config>(&consumer).await.unwrap();
    let mut batch = consumer
        .fetch()
        .max_messages(1)
        .expires(PROMPTLY)
        .messages()
        .await
        .unwrap();
    let redelivered = tokio::time::timeout(PROMPTLY, batch.next())
        .await
        .expect("redelivered before ack_wait")
        .expect("a redelivered command")
        .unwrap();
    assert!(redelivered.subject.ends_with(".session.s1.agent.load"));
    assert_eq!(redelivered.info().unwrap().delivered, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_command_in_flight_at_a_relaunch_is_redelivered_promptly() {
    assert_redelivered_when_the_run_ends(
        "redeliver_relaunch",
        |_, handshake| drop(handshake.relaunch_with(discord("1").into())),
        |exit| matches!(exit, RunExit::Reinitialize),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_command_in_flight_when_the_child_dies_is_redelivered_promptly() {
    assert_redelivered_when_the_run_ends(
        "redeliver_crash",
        |launcher, _| launcher.kill(),
        |exit| matches!(exit, RunExit::AgentExited | RunExit::AgentFailed(_)),
    )
    .await;
}
