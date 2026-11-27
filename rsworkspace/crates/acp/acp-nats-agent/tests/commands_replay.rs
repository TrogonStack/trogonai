//! The agent side must not replay acked COMMANDS when it restarts.
//!
//! COMMANDS is a `Limits` stream kept for days, so a consumer that started
//! from the beginning re-delivered every past prompt and cancel each time the
//! agent host relaunched.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use acp_nats::jetstream::provision::provision_streams;
use acp_nats::jetstream::streams::commands_stream_name;
use acp_nats::{AcpPrefix, AgentHandler};
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
};
use async_nats::jetstream;
use futures::StreamExt;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_nats::test_support::JetStreamTestServer;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
struct RecordingAgent {
    cancelled: Arc<Mutex<Vec<String>>>,
}

impl RecordingAgent {
    fn cancelled(&self) -> Vec<String> {
        self.cancelled.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait::async_trait]
impl AgentHandler for RecordingAgent {
    async fn initialize(&self, _args: InitializeRequest) -> agent_client_protocol::Result<InitializeResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> agent_client_protocol::Result<AuthenticateResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> agent_client_protocol::Result<NewSessionResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn prompt(&self, _args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn cancel(&self, args: CancelNotification) -> agent_client_protocol::Result<()> {
        self.cancelled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(args.session_id.to_string());
        Ok(())
    }
}

/// Publishes a `session/cancel` command to COMMANDS and returns its stream sequence.
async fn publish_cancel(
    js: &jetstream::Context,
    prefix: &AcpPrefix,
    session_id: &str,
) -> Result<u64, Box<dyn Error + Send + Sync>> {
    let encoded =
        acp_nats::wire::encode_notification("session/cancel", &CancelNotification::new(session_id.to_owned()))?;
    let subject = format!("{}.v1.session.{session_id}.agent.cancel", prefix.as_str());
    let ack = js
        .publish_with_headers(subject, encoded.headers, encoded.body)
        .await?
        .await?;
    Ok(ack.sequence)
}

/// Snapshots every consumer on COMMANDS. Not keyed on the durable name, so
/// the test also observes a consumer created without one.
async fn commands_consumers(
    js: &jetstream::Context,
    prefix: &AcpPrefix,
) -> Result<Vec<jetstream::consumer::Info>, Box<dyn Error + Send + Sync>> {
    let stream = js.get_stream(commands_stream_name(prefix)).await?;
    let mut consumers = stream.consumers();
    let mut infos = Vec::new();
    while let Some(info) = consumers.next().await {
        infos.push(info?);
    }
    Ok(infos)
}

async fn wait_for_consumer(js: &jetstream::Context, prefix: &AcpPrefix) -> TestResult {
    tokio::time::timeout(WAIT, async {
        while commands_consumers(js, prefix).await?.is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok::<_, Box<dyn Error + Send + Sync>>(())
    })
    .await?
}

/// Waits until the agent side has acked every command up to `sequence`, so the
/// next start must not see it again.
async fn wait_for_ack_floor(js: &jetstream::Context, prefix: &AcpPrefix, sequence: u64) -> TestResult {
    tokio::time::timeout(WAIT, async {
        loop {
            let infos = commands_consumers(js, prefix).await?;
            if infos
                .iter()
                .any(|info| info.ack_floor.stream_sequence >= sequence && info.num_ack_pending == 0)
            {
                return Ok::<_, Box<dyn Error + Send + Sync>>(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

async fn wait_for_cancels(agent: &RecordingAgent, count: usize) -> TestResult {
    tokio::time::timeout(WAIT, async {
        while agent.cancelled().len() < count {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    Ok(())
}

/// Runs one agent-side lifetime: starts the connection, runs `body`, then
/// stops it the way a host restart would.
async fn run_agent_side<F, Fut>(
    client: async_nats::Client,
    js: jetstream::Context,
    prefix: AcpPrefix,
    agent: RecordingAgent,
    body: F,
) -> TestResult
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = TestResult>,
{
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (_conn, io_task) = AgentSideNatsConnection::with_jetstream(agent, client, js, prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            let io = tokio::task::spawn_local(io_task);
            let result = body().await;
            io.abort();
            let _ = io.await;
            result
        })
        .await
}

#[tokio::test]
async fn restart_does_not_redeliver_acked_commands() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("replay")?;
    provision_streams(&NatsJetStreamClient::new(js.clone()), &prefix).await?;

    // A command already in the stream before the agent side ever ran is history,
    // not work: the first start must not pick it up.
    publish_cancel(&js, &prefix, "before-first-start").await?;

    let first = RecordingAgent::default();
    run_agent_side(client.clone(), js.clone(), prefix.clone(), first.clone(), || {
        let js = js.clone();
        let prefix = prefix.clone();
        let first = first.clone();
        async move {
            // The consumer is created asynchronously; wait for it before publishing.
            wait_for_consumer(&js, &prefix).await?;
            let sequence = publish_cancel(&js, &prefix, "first-run").await?;
            wait_for_cancels(&first, 1).await?;
            wait_for_ack_floor(&js, &prefix, sequence).await
        }
    })
    .await?;
    assert_eq!(first.cancelled(), ["first-run"]);

    // Published while no agent side is running: kept for the next start.
    publish_cancel(&js, &prefix, "while-down").await?;

    let second = RecordingAgent::default();
    run_agent_side(client, js.clone(), prefix.clone(), second.clone(), || {
        let js = js.clone();
        let prefix = prefix.clone();
        let second = second.clone();
        async move {
            let sequence = publish_cancel(&js, &prefix, "second-run").await?;
            wait_for_cancels(&second, 2).await?;
            wait_for_ack_floor(&js, &prefix, sequence).await?;
            // Give any stray redelivery of `first-run` time to show up.
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
    })
    .await?;
    assert_eq!(second.cancelled(), ["while-down", "second-run"]);
    let consumers = commands_consumers(&js, &prefix).await?;
    assert_eq!(consumers.len(), 1, "restarts must reuse one durable consumer");
    Ok(())
}
