//! Real-server coverage for a prompt whose response consumer disappears mid-turn.
//!
//! A network stall longer than the server's inactivity threshold deletes the
//! bridge's ephemeral response consumer while the runner keeps working. The
//! runner's answer then lands on the responses stream while no consumer exists.
//! Only a real JetStream server shows whether the recreated consumer rereads it.

use std::error::Error;
use std::time::Duration;

use acp_nats::jetstream::provision::provision_streams;
use acp_nats::jetstream::streams::responses_stream_name;
use acp_nats::nats::commands::PromptSubject;
use acp_nats::wire::{encode_success, response_id_from_request_headers};
use acp_nats::{
    AcpPrefix, AcpSessionId, AgentHandler, Bridge, Config, NatsAuth, NatsConfig, NatsJetStreamClient, ResponseSubject,
};
use agent_client_protocol::v1::schema::{PromptRequest, PromptResponse, StopReason};
use futures::StreamExt;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::time::SystemClock;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

const PREFIX: &str = "resume";
const SESSION: &str = "session-1";

#[tokio::test]
async fn prompt_returns_the_runner_answer_published_while_its_response_consumer_was_gone() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let context = server.jetstream().await;
    let prefix = AcpPrefix::new(PREFIX)?;
    let session_id = AcpSessionId::new(SESSION)?;
    let js = NatsJetStreamClient::new(context.clone());
    provision_streams(&js, &prefix).await?;

    let mut prompts = nats.subscribe(PromptSubject::new(&prefix, &session_id)).await?;
    let config = Config::new(
        prefix.clone(),
        NatsConfig::new(vec![server.address().to_string()], NatsAuth::None),
    )
    .with_prompt_timeout(Duration::from_secs(30));
    let bridge = Bridge::new(
        nats.clone(),
        js,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        config,
    );
    let prompt = tokio::spawn(async move { bridge.prompt(PromptRequest::new(SESSION, vec![])).await });

    let request = tokio::time::timeout(Duration::from_secs(10), prompts.next())
        .await?
        .ok_or("prompt subscription ended")?;
    let request_id = response_id_from_request_headers(&request.headers.unwrap_or_default());

    // Simulate the stall outcome: the server drops the ephemeral consumer...
    let responses = context.get_stream(responses_stream_name(&prefix)).await?;
    let consumer_names = responses.consumer_names().collect::<Vec<_>>().await;
    let [Ok(consumer_name)] = consumer_names.as_slice() else {
        return Err(format!("expected the prompt's one response consumer, found {consumer_names:?}").into());
    };
    responses.delete_consumer(consumer_name).await?;

    // ...and the runner answers before the bridge has a consumer again.
    let answer = encode_success(request_id, &PromptResponse::new(StopReason::EndTurn))?;
    context
        .publish_with_headers(ResponseSubject::new(&prefix, &session_id), answer.headers, answer.body)
        .await?
        .await?;

    let response = tokio::time::timeout(Duration::from_secs(20), prompt).await???;
    assert_eq!(response.stop_reason, StopReason::EndTurn);
    Ok(())
}
