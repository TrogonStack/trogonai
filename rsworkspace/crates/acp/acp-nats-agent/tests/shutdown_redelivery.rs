//! A command the agent side holds when it stops is handed back at once, not
//! left to wait out `ack_wait` on a host that is going away.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use acp_nats::jetstream::provision::provision_selected_streams;
use acp_nats::jetstream::streams::commands_stream_name;
use acp_nats::nats::AcpStream;
use acp_nats::wire::encode_request;
use acp_nats::{AcpPrefix, AgentHandler};
use acp_nats_agent::{AdmitAll, AgentSideNatsConnection};
use agent_client_protocol::Result;
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    StopReason,
};
use async_nats::jetstream::consumer::{PullConsumer, pull};
use async_nats::jetstream::stream::Stream;
use futures::StreamExt;
use jsonrpc_nats::RequestId;
use tokio::sync::Notify;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_nats::test_support::JetStreamTestServer;

/// Well under the consumer's 30 s `ack_wait`: a command that is only
/// redelivered after `ack_wait` cannot arrive inside it.
const PROMPTLY: Duration = Duration::from_secs(5);

/// The command the host is working on when it stops: it never finishes. Its
/// clones share the counters, so the test keeps one while the connection
/// owns another.
#[derive(Clone, Default)]
struct StuckLoad {
    loads: Arc<AtomicU32>,
    started: Arc<Notify>,
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
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        std::future::pending().await
    }
}

/// Waits until the agent side has created its consumer on COMMANDS. A durable
/// consumer that only delivers new commands would skip one published before
/// it exists.
async fn wait_for_commands_consumer(stream: &mut Stream) {
    tokio::time::timeout(PROMPTLY, async {
        while stream.info().await.unwrap().state.consumer_count == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent side creates its COMMANDS consumer");
}

#[tokio::test(flavor = "current_thread")]
async fn a_command_in_flight_at_stop_is_redelivered_promptly() {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let jetstream = async_nats::jetstream::new(nats.clone());
    let js = NatsJetStreamClient::new(jetstream.clone());
    let prefix = AcpPrefix::new("shutdown").unwrap();
    provision_selected_streams(&js, &prefix, [AcpStream::Commands, AcpStream::Responses])
        .await
        .expect("provision COMMANDS and RESPONSES");
    let mut stream = jetstream.get_stream(commands_stream_name(&prefix)).await.unwrap();

    let agent = StuckLoad::default();
    let stop = Rc::new(Notify::new());
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_connection, serving) = AgentSideNatsConnection::with_jetstream_gated_until(
                agent.clone(),
                AdmitAll,
                nats.clone(),
                js.clone(),
                prefix.clone(),
                |task| {
                    tokio::task::spawn_local(task);
                },
                {
                    let stop = stop.clone();
                    async move { stop.notified().await }
                },
            );
            let serving = tokio::task::spawn_local(serving);
            wait_for_commands_consumer(&mut stream).await;

            let command = encode_request(
                "session/load",
                RequestId::String("load-1".into()),
                &LoadSessionRequest::new("s1", "/tmp"),
            )
            .unwrap();
            jetstream
                .publish_with_headers("shutdown.v1.session.s1.agent.load", command.headers, command.body)
                .await
                .unwrap()
                .await
                .expect("COMMANDS stores the command");
            tokio::time::timeout(PROMPTLY, agent.started.notified())
                .await
                .expect("the agent side starts on the command");

            stop.notify_one();
            tokio::time::timeout(PROMPTLY, serving)
                .await
                .expect("serving settles its commands and returns")
                .unwrap()
                .expect("serving ends cleanly");
        })
        .await;
    nats.flush().await.unwrap();

    // The next puller on the same consumer gets the command straight away.
    let mut names = stream.consumer_names();
    let name = names.next().await.expect("the agent side's consumer").unwrap();
    assert!(names.next().await.is_none(), "exactly one consumer on COMMANDS");
    let consumer: PullConsumer = stream.get_consumer::<pull::Config>(&name).await.unwrap();
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
    assert_eq!(redelivered.subject.as_str(), "shutdown.v1.session.s1.agent.load");
    assert_eq!(redelivered.info().unwrap().delivered, 2);
    assert_eq!(agent.loads.load(Ordering::SeqCst), 1);
}
