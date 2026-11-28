//! Real-server coverage for the response consumers a session request leaves behind.
//!
//! Every session request creates an ephemeral consumer on the RESPONSES stream
//! before it publishes. Whatever way the request ends, that consumer has to go
//! with it, or a busy bridge piles consumers up on the stream.

use std::error::Error;
use std::time::Duration;

use acp_nats::jetstream::provision::provision_streams;
use acp_nats::jetstream::streams::responses_stream_name;
use acp_nats::wire::{encode_agent_error, encode_success, response_id_from_request_headers};
use acp_nats::{
    AcpPrefix, AcpSessionId, AgentHandler, Bridge, Config, NatsAuth, NatsConfig, NatsJetStreamClient, ResponseSubject,
};
use agent_client_protocol::v1::schema::{SetSessionModeRequest, SetSessionModeResponse};
use agent_client_protocol::{Error as AcpError, ErrorCode};
use futures::StreamExt;
use futures::future::join_all;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::time::SystemClock;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

const PREFIX: &str = "cleanup";
const PER_PATH: usize = 5;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(1);
const CLEANUP_GRACE: Duration = Duration::from_secs(1);

/// How the fake runner treats a session, chosen by the session id's prefix.
const ANSWERED: &str = "ok";
const REJECTED: &str = "err";
const UNANSWERED: &str = "silent";

/// Answers, rejects, or ignores each command, chosen by its session.
async fn run_runner(
    mut commands: async_nats::Subscriber,
    context: async_nats::jetstream::Context,
    prefix: AcpPrefix,
) -> TestResult {
    while let Some(command) = commands.next().await {
        let session = command.subject.split('.').nth(3).unwrap_or_default().to_owned();
        let id = response_id_from_request_headers(&command.headers.unwrap_or_default());
        let answer = if session.starts_with(ANSWERED) {
            encode_success(id, &SetSessionModeResponse::new())?
        } else if session.starts_with(REJECTED) {
            encode_agent_error(id, &AcpError::new(ErrorCode::INTERNAL_ERROR, "runner failed"))
        } else {
            continue;
        };
        let session_id = AcpSessionId::new(&session)?;
        context
            .publish_with_headers(ResponseSubject::new(&prefix, &session_id), answer.headers, answer.body)
            .await?
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn session_requests_leave_no_response_consumers_behind() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let context = server.jetstream().await;
    let prefix = AcpPrefix::new(PREFIX)?;
    let js = NatsJetStreamClient::new(context.clone());
    provision_streams(&js, &prefix).await?;

    let commands = nats.subscribe(format!("{PREFIX}.v1.session.*.agent.>")).await?;
    let runner = tokio::spawn(run_runner(commands, context.clone(), prefix.clone()));

    let config = Config::new(
        prefix.clone(),
        NatsConfig::new(vec![server.address().to_string()], NatsAuth::None),
    )
    .with_operation_timeout(OPERATION_TIMEOUT);
    let bridge = Bridge::new(
        nats.clone(),
        js,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-test"),
        config,
    );
    let set_mode = |session: String| bridge.set_session_mode(SetSessionModeRequest::new(session, "code"));
    let batch = |kind: &'static str| join_all((0..PER_PATH).map(move |i| set_mode(format!("{kind}-{i}"))));

    // All four ways a request ends, at once. A caller that stops waiting drops
    // the request future mid-flight.
    let (answered, rejected, timed_out, dropped) = tokio::join!(
        batch(ANSWERED),
        batch(REJECTED),
        batch(UNANSWERED),
        tokio::time::timeout(OPERATION_TIMEOUT / 4, batch(UNANSWERED)),
    );
    assert!(answered.iter().all(Result::is_ok), "{answered:?}");
    assert!(
        rejected
            .iter()
            .all(|r| r.as_ref().is_err_and(|e| e.message == "runner failed")),
        "{rejected:?}"
    );
    assert!(
        timed_out
            .iter()
            .all(|r| r.as_ref().is_err_and(|e| e.message.contains("timed out"))),
        "{timed_out:?}"
    );
    assert!(dropped.is_err(), "the dropped requests should still have been pending");

    // Deletes run in the background: allow a moment, but far less than the
    // server's 5 s inactivity threshold, which would otherwise hide the leak.
    let responses = context.get_stream(responses_stream_name(&prefix)).await?;
    let deadline = tokio::time::Instant::now() + CLEANUP_GRACE;
    let left = loop {
        let left = responses.consumer_names().collect::<Vec<_>>().await.len();
        if left == 0 || tokio::time::Instant::now() >= deadline {
            break left;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    runner.abort();
    assert_eq!(
        left, 0,
        "response consumers left on the stream after every request ended"
    );
    Ok(())
}
