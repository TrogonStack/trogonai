use super::*;
use async_nats::jetstream::message::StreamMessage;
use bytes::Bytes;
use std::time::Duration;
use time::OffsetDateTime;
use trogon_nats::MockNatsClient;
use trogon_nats::jetstream::MockJetStreamConsumerFactory;
use trogon_nats::test_support::JetStreamTestServer;

#[tokio::test]
async fn notion_verification_token_command_reads_latest_message() {
    let resolved = resolved_config();
    let nats = MockNatsClient::new();
    let js = MockJetStreamConsumerFactory::new();
    js.add_last_raw_message(stream_message(
        "notion-primary.subscription.verification",
        Bytes::from_static(br#"{"verification_token":"secret_token"}"#),
    ));
    let mut out = Vec::new();

    notion_verification_token(&resolved, &integration("primary"), false, &nats, &js, &mut out)
        .await
        .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "secret_token\n");
    assert!(nats.subscribed_to().is_empty());
    assert_eq!(js.get_stream_calls(), vec!["NOTION_PRIMARY"]);
    assert_eq!(
        js.last_raw_message_subjects(),
        vec!["notion-primary.subscription.verification"]
    );
}

#[tokio::test]
async fn notion_verification_token_command_watches_subscription() {
    let resolved = resolved_config();
    let nats = MockNatsClient::new();
    let messages = nats.inject_messages();
    messages
        .unbounded_send(nats_message(
            "notion-primary.subscription.verification",
            br#"{"verification_token":"watched_token"}"#,
        ))
        .unwrap();
    drop(messages);
    let js = MockJetStreamConsumerFactory::new();
    let mut out = Vec::new();

    notion_verification_token(&resolved, &integration("primary"), true, &nats, &js, &mut out)
        .await
        .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "watched_token\n");
    assert_eq!(nats.subscribed_to(), vec!["notion-primary.subscription.verification"]);
    assert!(js.get_stream_calls().is_empty());
}

#[tokio::test]
async fn notion_verification_token_command_rejects_unknown_integration_before_using_deps() {
    let resolved = resolved_config();
    let nats = MockNatsClient::new();
    let js = MockJetStreamConsumerFactory::new();
    let mut out = Vec::new();

    let error = notion_verification_token(&resolved, &integration("missing"), false, &nats, &js, &mut out)
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "notion integration 'missing' is not configured");
    assert!(error.downcast_ref::<NotionVerificationTokenCommandError>().is_some());
    assert!(out.is_empty());
    assert!(nats.subscribed_to().is_empty());
    assert!(js.get_stream_calls().is_empty());
}

fn resolved_config() -> config::ResolvedConfig {
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    write!(
        file,
        r#"
[sources.notion.integrations.primary.webhook]
verification_token = "configured-token"
"#
    )
    .unwrap();

    config::load(Some(file.path())).unwrap()
}

fn integration(value: &str) -> source_integration_id::SourceIntegrationId {
    source_integration_id::SourceIntegrationId::new(value).unwrap()
}

fn nats_message(subject: &str, payload: &'static [u8]) -> async_nats::Message {
    async_nats::Message {
        subject: subject.into(),
        reply: None,
        payload: Bytes::from_static(payload),
        headers: None,
        status: None,
        description: None,
        length: payload.len(),
    }
}

fn stream_message(subject: &str, payload: Bytes) -> StreamMessage {
    StreamMessage {
        subject: subject.into(),
        sequence: 1,
        headers: async_nats::HeaderMap::new(),
        payload,
        time: OffsetDateTime::UNIX_EPOCH,
    }
}

#[tokio::test]
async fn serve_rejects_empty_sources_before_connecting() {
    let file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    let resolved = config::load(Some(file.path())).unwrap();
    let error = serve(resolved, std::future::pending()).await.unwrap_err();
    assert!(error.to_string().starts_with("no sources configured"));
}

#[tokio::test]
async fn serve_provisions_stream_and_claim_bucket_before_graceful_shutdown() {
    let server = JetStreamTestServer::start().await;
    let mut resolved = resolved_config();
    resolved.http_server.port = 0;
    resolved.nats = trogon_nats::NatsConfig::from_url(server.address());
    tokio::time::timeout(Duration::from_secs(10), serve(resolved, std::future::ready(())))
        .await
        .unwrap()
        .unwrap();
    let js = server.jetstream().await;
    let mut stream = js.get_stream("NOTION_PRIMARY").await.unwrap();
    assert_eq!(stream.info().await.unwrap().config.subjects, ["notion-primary.>"]);
    NatsObjectStore::bind_claim_bucket(&js, ClaimBucket::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn serve_stops_slack_and_http_on_the_shared_shutdown() {
    let server = JetStreamTestServer::start().await;
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    write!(
        file,
        r#"
[sources.slack.integrations.primary.socket_mode]
app_token = "xapp-fixture-token"
"#
    )
    .unwrap();
    let mut resolved = config::load(Some(file.path())).unwrap();
    resolved.http_server.port = 0;
    resolved.nats = trogon_nats::NatsConfig::from_url(server.address());
    tokio::time::timeout(Duration::from_secs(10), serve(resolved, std::future::ready(())))
        .await
        .unwrap()
        .unwrap();
    let js = server.jetstream().await;
    js.get_stream("SLACK_PRIMARY").await.unwrap();
}

#[tokio::test]
async fn serve_propagates_connection_errors() {
    let directory = tempfile::tempdir().unwrap();
    let mut resolved = resolved_config();
    resolved.nats.auth = trogon_nats::NatsAuth::Credentials(directory.path().join("missing.creds"));
    let error = tokio::time::timeout(Duration::from_secs(5), serve(resolved, std::future::pending()))
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<trogon_nats::ConnectError>(),
        Some(trogon_nats::ConnectError::InvalidCredentials(_))
    ));
}

#[tokio::test]
async fn source_supervisor_tolerates_one_failed_source_when_another_stops_cleanly() {
    let mut tasks = JoinSet::new();
    tasks.spawn(async { ("healthy", Ok(())) });
    tasks.spawn(async { ("failed", Err(anyhow::anyhow!("source refused startup"))) });
    wait_for_sources(tasks).await.unwrap();
}

#[tokio::test]
async fn source_supervisor_reports_errors_and_panics_when_all_sources_fail() {
    let mut tasks = JoinSet::new();
    tasks.spawn(async { ("failed", Err(anyhow::anyhow!("source refused startup"))) });
    tasks.spawn(async { panic!("source task crashed") });
    let error = wait_for_sources(tasks).await.unwrap_err();
    assert_eq!(error.to_string(), "all 2 task(s) failed");
}

#[tokio::test]
async fn notion_verification_token_preserves_output_write_failures() {
    let resolved = resolved_config();
    let nats = MockNatsClient::new();
    let js = MockJetStreamConsumerFactory::new();
    js.add_last_raw_message(stream_message(
        "notion-primary.subscription.verification",
        Bytes::from_static(br#"{"verification_token":"secret_token"}"#),
    ));
    let mut output = std::io::Cursor::new([]);
    let error = notion_verification_token(&resolved, &integration("primary"), false, &nats, &js, &mut output)
        .await
        .unwrap_err();
    assert!(error.downcast_ref::<std::io::Error>().is_some());
}
