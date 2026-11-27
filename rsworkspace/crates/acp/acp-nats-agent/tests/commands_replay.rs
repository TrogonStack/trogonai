//! The agent side must run each COMMANDS entry once.
//!
//! COMMANDS is a `Limits` stream kept for days, so a consumer that started
//! from the beginning re-delivered every past prompt and cancel each time the
//! agent host relaunched. JetStream also redelivers a command whose acks are
//! lost (a partition longer than `ack_wait`) while the host is still running
//! it, and the consumer or the stream can disappear under a running host.

use std::error::Error;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use acp_nats::jetstream::consumers::{COMMANDS_CONSUMER_DURABLE, agent_commands_consumer};
use acp_nats::jetstream::provision::provision_streams;
use acp_nats::jetstream::streams::commands_stream_name;
use acp_nats::{AcpPrefix, AgentHandler};
use acp_nats_agent::{AgentSideNatsConnection, ConnectionError};
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, StopReason,
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
    prompts: Arc<AtomicUsize>,
    /// How long each prompt runs before it answers.
    prompt_hold: Duration,
}

impl RecordingAgent {
    fn holding_prompts(prompt_hold: Duration) -> Self {
        Self {
            prompt_hold,
            ..Self::default()
        }
    }

    fn cancelled(&self) -> Vec<String> {
        self.cancelled.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn prompts(&self) -> usize {
        self.prompts.load(Ordering::SeqCst)
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
        self.prompts.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.prompt_hold).await;
        Ok(PromptResponse::new(StopReason::EndTurn))
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

/// Publishes a `session/prompt` request for session `s1` to COMMANDS.
async fn publish_prompt(js: &jetstream::Context, prefix: &AcpPrefix, id: &str) -> TestResult {
    let encoded = acp_nats::wire::encode_request(
        "session/prompt",
        jsonrpc_nats::RequestId::String(id.to_owned()),
        &PromptRequest::new("s1", Vec::new()),
    )?;
    let subject = format!("{}.v1.session.s1.agent.prompt", prefix.as_str());
    js.publish_with_headers(subject, encoded.headers, encoded.body)
        .await?
        .await?;
    Ok(())
}

/// Shortens the durable's `ack_wait` under the running agent side, so a
/// prompt outlives it without waiting minutes. Same effect as acks lost in a
/// partition: the 15 s keepalive cannot reset a 1 s `ack_wait` in time.
async fn shorten_ack_wait(js: &jetstream::Context, prefix: &AcpPrefix) -> TestResult {
    let stream = js.get_stream(commands_stream_name(prefix)).await?;
    stream
        .create_consumer(jetstream::consumer::pull::Config {
            ack_wait: SHORT_ACK_WAIT,
            ..agent_commands_consumer()
        })
        .await?;
    Ok(())
}

const SHORT_ACK_WAIT: Duration = Duration::from_secs(1);

/// Polls `done` until it holds, failing after [`WAIT`].
async fn wait_until<F, Fut>(mut done: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, Box<dyn Error + Send + Sync>>>,
{
    tokio::time::timeout(WAIT, async {
        while !done().await? {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    })
    .await?
}

async fn wait_for_prompts(agent: &RecordingAgent, count: usize) -> TestResult {
    wait_until(|| async { Ok(agent.prompts() >= count) }).await
}

/// Collects every response published for session `s1` within `window`.
async fn responses_within(responses: &mut async_nats::Subscriber, window: Duration) -> Vec<async_nats::Message> {
    let mut received = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(Some(message)) = tokio::time::timeout_at(deadline, responses.next()).await {
        received.push(message);
    }
    received
}

async fn subscribe_responses(
    client: &async_nats::Client,
    prefix: &AcpPrefix,
) -> Result<async_nats::Subscriber, Box<dyn Error + Send + Sync>> {
    let subscriber = client
        .subscribe(format!("{}.v1.session.s1.agent.response", prefix.as_str()))
        .await?;
    client.flush().await?;
    Ok(subscriber)
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
    wait_until(|| async { Ok(!commands_consumers(js, prefix).await?.is_empty()) }).await
}

/// Waits until the agent side has acked every command up to `sequence`, so the
/// next start must not see it again.
async fn wait_for_ack_floor(js: &jetstream::Context, prefix: &AcpPrefix, sequence: u64) -> TestResult {
    wait_until(|| async {
        Ok(commands_consumers(js, prefix)
            .await?
            .iter()
            .any(|info| info.ack_floor.stream_sequence >= sequence && info.num_ack_pending == 0))
    })
    .await
}

async fn wait_for_cancels(agent: &RecordingAgent, count: usize) -> TestResult {
    wait_until(|| async { Ok(agent.cancelled().len() >= count) }).await
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

#[tokio::test]
async fn redelivery_of_a_running_prompt_is_not_run_twice() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("inflight")?;
    provision_streams(&NatsJetStreamClient::new(js.clone()), &prefix).await?;
    let mut responses = subscribe_responses(&client, &prefix).await?;

    // The prompt runs through several ack_wait periods.
    let agent = RecordingAgent::holding_prompts(SHORT_ACK_WAIT * 4);
    run_agent_side(client.clone(), js.clone(), prefix.clone(), agent.clone(), || {
        let js = js.clone();
        let prefix = prefix.clone();
        let agent = agent.clone();
        let responses = &mut responses;
        async move {
            wait_for_consumer(&js, &prefix).await?;
            shorten_ack_wait(&js, &prefix).await?;
            publish_prompt(&js, &prefix, "prompt-1").await?;
            wait_for_prompts(&agent, 1).await?;

            // JetStream must redeliver the running prompt for this test to mean
            // anything. Checked while it runs: the counter only covers pending
            // messages.
            wait_until(|| async {
                Ok(commands_consumers(&js, &prefix)
                    .await?
                    .iter()
                    .any(|info| info.num_redelivered > 0))
            })
            .await?;

            let received = responses_within(responses, SHORT_ACK_WAIT * 4).await;
            assert_eq!(agent.prompts(), 1, "the redelivery must not reach the agent");
            assert_eq!(received.len(), 1, "exactly one response for the request id");
            Ok(())
        }
    })
    .await
}

#[tokio::test]
async fn command_left_unsettled_at_shutdown_runs_once_on_the_next_host() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("handover")?;
    provision_streams(&NatsJetStreamClient::new(js.clone()), &prefix).await?;
    let mut responses = subscribe_responses(&client, &prefix).await?;

    // The first host stops while the prompt is running, without settling it.
    let first = RecordingAgent::holding_prompts(Duration::from_secs(600));
    run_agent_side(client.clone(), js.clone(), prefix.clone(), first.clone(), || {
        let js = js.clone();
        let prefix = prefix.clone();
        let first = first.clone();
        async move {
            wait_for_consumer(&js, &prefix).await?;
            shorten_ack_wait(&js, &prefix).await?;
            publish_prompt(&js, &prefix, "prompt-1").await?;
            wait_for_prompts(&first, 1).await
        }
    })
    .await?;
    // Let ack_wait run out with no host holding the prompt. The next host
    // restores the long ack_wait when it starts, so it must not start sooner.
    tokio::time::sleep(SHORT_ACK_WAIT * 2).await;

    let second = RecordingAgent::default();
    run_agent_side(client.clone(), js.clone(), prefix.clone(), second.clone(), || {
        let second = second.clone();
        let responses = &mut responses;
        async move {
            wait_for_prompts(&second, 1).await?;
            let received = responses_within(responses, Duration::from_secs(2)).await;
            assert_eq!(received.len(), 1, "the next host answers the prompt once");
            Ok(())
        }
    })
    .await?;
    assert_eq!(first.prompts(), 1);
    assert_eq!(second.prompts(), 1);
    Ok(())
}

#[tokio::test]
async fn deleted_consumer_is_recreated_under_a_running_agent_side() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("lostconsumer")?;
    provision_streams(&NatsJetStreamClient::new(js.clone()), &prefix).await?;

    let agent = RecordingAgent::default();
    run_agent_side(client, js.clone(), prefix.clone(), agent.clone(), || {
        let js = js.clone();
        let prefix = prefix.clone();
        let agent = agent.clone();
        async move {
            wait_for_consumer(&js, &prefix).await?;
            let stream = js.get_stream(commands_stream_name(&prefix)).await?;
            stream.delete_consumer(COMMANDS_CONSUMER_DURABLE).await?;

            // The recreated durable starts at new commands, so wait for it first.
            wait_until(|| async { Ok(stream.consumer_info(COMMANDS_CONSUMER_DURABLE).await.is_ok()) }).await?;
            let sequence = publish_cancel(&js, &prefix, "after-recreate").await?;
            wait_for_cancels(&agent, 1).await?;
            wait_for_ack_floor(&js, &prefix, sequence).await
        }
    })
    .await?;
    assert_eq!(agent.cancelled(), ["after-recreate"]);
    Ok(())
}

#[tokio::test]
async fn deleted_stream_ends_serving_with_an_error() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    let prefix = AcpPrefix::new("loststream")?;
    provision_streams(&NatsJetStreamClient::new(js.clone()), &prefix).await?;

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async {
            let (_conn, io_task) = AgentSideNatsConnection::with_jetstream(
                RecordingAgent::default(),
                client,
                js.clone(),
                prefix.clone(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            let io = tokio::task::spawn_local(io_task);
            wait_for_consumer(&js, &prefix).await?;
            js.delete_stream(commands_stream_name(&prefix)).await?;
            Ok::<_, Box<dyn Error + Send + Sync>>(tokio::time::timeout(WAIT, io).await??)
        })
        .await?;

    assert!(
        matches!(result, Err(ConnectionError::CommandsStreamMissing { .. })),
        "{result:?}"
    );
    Ok(())
}
