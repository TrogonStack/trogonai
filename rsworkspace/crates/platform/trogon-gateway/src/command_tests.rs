use super::*;
use async_nats::jetstream::message::StreamMessage;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use time::OffsetDateTime;
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamEvent, StreamRead};
use trogon_nats::MockNatsClient;
use trogon_nats::jetstream::{MockJetStreamConsumerFactory, MockJetStreamPublishMessage};
use trogon_nats::test_support::JetStreamTestServer;

#[derive(Clone, Default)]
struct EmptyCredentialEventStore {
    events: Arc<Mutex<Vec<StreamEvent>>>,
}

#[derive(Debug, thiserror::Error)]
#[error("empty credential event store failed")]
struct EmptyCredentialEventStoreError;

impl StreamRead<str> for EmptyCredentialEventStore {
    type Error = EmptyCredentialEventStoreError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let start = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let events = self.events.lock().unwrap();
        Ok(ReadStreamResponse {
            current_position: events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id)
                .map(|event| event.stream_position)
                .max(),
            events: events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                .cloned()
                .collect(),
        })
    }
}

#[tokio::test]
async fn credential_runtime_projection_refresh_handles_empty_stream() {
    let stream = MockJetStreamPublishMessage::new();
    let event_store = EmptyCredentialEventStore::default();
    let registry = credential_runtime_projection::RuntimeCredentialRegistry::default();

    let (_registry, report) = refresh_credential_runtime_projection_from_start(&stream, &event_store, registry)
        .await
        .unwrap();

    assert_eq!(report.scanned_events(), 0);
    assert_eq!(report.decoded_events(), 0);
    assert_eq!(report.projected_integrations(), 0);
    assert_eq!(report.checkpoint_loaded_sequence(), 0);
    assert_eq!(report.checkpoint_advanced_to(), None);
}

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
intended_use = "development"
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
async fn serve_provisions_and_stops_google_pubsub_on_the_shared_shutdown() {
    let server = JetStreamTestServer::start().await;
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    file.write_all(
        br#"
[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
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
    js.get_stream("GOOGLE_PUBSUB_PRIMARY").await.unwrap();
}

#[tokio::test]
async fn serve_fails_fast_naming_the_integration_whose_credentials_cannot_be_built() {
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    file.write_all(
        br#"
[sources.google_pubsub.integrations.orders.streaming_pull]
subscription = "projects/p/subscriptions/orders"
credentials = '{"type":"service_account","client_email":"leaky-marker@example.invalid"}'
"#,
    )
    .unwrap();
    let resolved = config::load(Some(file.path())).unwrap();

    let error = tokio::time::timeout(Duration::from_secs(5), serve(resolved, std::future::pending()))
        .await
        .unwrap()
        .unwrap_err()
        .to_string();

    assert!(error.contains("google_pubsub/orders"));
    assert!(!error.contains("leaky-marker"));
}

#[tokio::test]
async fn serve_propagates_connection_errors() {
    let directory = tempfile::tempdir().unwrap();
    let mut resolved = resolved_config();
    resolved.nats.auth =
        trogon_nats::NatsAuth::Credentials(trogon_nats::Location::Path(directory.path().join("missing.creds")));
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

fn management_env(address: &str) -> trogon_std::env::InMemoryEnv {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(CREDENTIAL_MANAGEMENT_ADMIN_TOKEN, "admin-token");
    env.set(OPENBAO_ADDR, address);
    env.set(OPENBAO_TOKEN, "openbao-token");
    env.set(OPENBAO_ALLOW_PLAINTEXT, "true");
    env
}

struct NonUnicodeEnv;

impl ReadEnv for NonUnicodeEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotUnicode(std::ffi::OsString::new()))
    }
}

#[tokio::test]
async fn serve_enables_credential_management_when_the_admin_token_is_configured() {
    let server = JetStreamTestServer::start().await;
    let mut resolved = resolved_config();
    resolved.http_server.port = 0;
    resolved.nats = trogon_nats::NatsConfig::from_url(server.address());
    let env = management_env("http://127.0.0.1:1");

    tokio::time::timeout(
        Duration::from_secs(10),
        serve_with_env(resolved, std::future::ready(()), &env),
    )
    .await
    .unwrap()
    .unwrap();

    let js = server.jetstream().await;
    js.get_key_value(credential_management::idempotency::bucket_config().bucket)
        .await
        .unwrap();
}

#[tokio::test]
async fn serve_does_not_provision_credential_management_without_the_admin_token() {
    let server = JetStreamTestServer::start().await;
    let mut resolved = resolved_config();
    resolved.http_server.port = 0;
    resolved.nats = trogon_nats::NatsConfig::from_url(server.address());

    tokio::time::timeout(
        Duration::from_secs(10),
        serve_with_env(resolved, std::future::ready(()), &trogon_std::env::InMemoryEnv::new()),
    )
    .await
    .unwrap()
    .unwrap();

    let js = server.jetstream().await;
    assert!(
        js.get_key_value(credential_management::idempotency::bucket_config().bucket)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn serve_mounts_runtime_credentials_and_spawns_their_workers_for_flagged_sources() {
    let server = JetStreamTestServer::start().await;
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    write!(
        file,
        r#"
[sources.telegram.integrations.first.webhook]
webhook_secret = "secret-first"
webhook_registration_mode = "startup"
bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ"
public_webhook_url = "https://example.com/sources/telegram/first/webhook"

[sources.discord]
gateway_intents = "guilds,guild_messages"
"#
    )
    .unwrap();
    let mut resolved = config::load(Some(file.path())).unwrap();
    resolved.http_server.port = 0;
    resolved.nats = trogon_nats::NatsConfig::from_url(server.address());
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(OPENBAO_ADDR, "http://127.0.0.1:1");
    env.set(OPENBAO_TOKEN, "openbao-token");
    env.set(OPENBAO_ALLOW_PLAINTEXT, "true");
    env.set(ENABLE_RUNTIME_TELEGRAM_CREDENTIALS, "true");
    env.set(ENABLE_RUNTIME_DISCORD_CREDENTIALS, "true");

    tokio::time::timeout(
        Duration::from_secs(10),
        serve_with_env(resolved, std::future::ready(()), &env),
    )
    .await
    .unwrap()
    .unwrap();

    let js = server.jetstream().await;
    js.get_stream("TELEGRAM_FIRST").await.unwrap();
}

const EVERY_WEBHOOK_SOURCE: &str = r#"
[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"

[sources.telegram.integrations.primary.webhook]
webhook_secret = "tg-secret"

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"

[sources.gitlab.integrations.primary.webhook]
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="

[sources.incidentio.integrations.primary.webhook]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="

[sources.linear.integrations.primary.webhook]
webhook_secret = "linear-secret"

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"

[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"

[sources.zoom.integrations.primary.webhook]
secret_token = "zoom-secret-token"

[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
"#;

async fn serve_every_webhook_source(env: &trogon_std::env::InMemoryEnv) {
    let server = JetStreamTestServer::start().await;
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    file.write_all(EVERY_WEBHOOK_SOURCE.as_bytes()).unwrap();
    let mut resolved = config::load(Some(file.path())).unwrap();
    resolved.http_server.port = 0;
    resolved.nats = trogon_nats::NatsConfig::from_url(server.address());

    tokio::time::timeout(
        Duration::from_secs(10),
        serve_with_env(resolved, std::future::ready(()), env),
    )
    .await
    .unwrap()
    .unwrap();

    let js = server.jetstream().await;
    for stream in ["GITHUB_PRIMARY", "SLACK_PRIMARY", "ZOOM_PRIMARY", "PLAID_PRIMARY"] {
        js.get_stream(stream).await.unwrap();
    }
}

#[tokio::test]
async fn serve_mounts_every_webhook_source_with_static_secrets() {
    serve_every_webhook_source(&trogon_std::env::InMemoryEnv::new()).await;
}

#[tokio::test]
async fn serve_mounts_every_webhook_source_with_runtime_credentials() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(OPENBAO_ADDR, "http://127.0.0.1:1");
    env.set(OPENBAO_TOKEN, "openbao-token");
    env.set(OPENBAO_ALLOW_PLAINTEXT, "true");
    for flag in [
        ENABLE_RUNTIME_GITHUB_CREDENTIALS,
        ENABLE_RUNTIME_GITLAB_CREDENTIALS,
        ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS,
        ENABLE_RUNTIME_LINEAR_CREDENTIALS,
        ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS,
        ENABLE_RUNTIME_NOTION_CREDENTIALS,
        ENABLE_RUNTIME_SENTRY_CREDENTIALS,
        ENABLE_RUNTIME_SLACK_CREDENTIALS,
        ENABLE_RUNTIME_TELEGRAM_CREDENTIALS,
        ENABLE_RUNTIME_TWITTER_CREDENTIALS,
    ] {
        env.set(flag, "true");
    }

    serve_every_webhook_source(&env).await;
}

#[tokio::test]
async fn serve_requires_openbao_settings_when_credential_management_is_enabled() {
    let server = JetStreamTestServer::start().await;
    let mut resolved = resolved_config();
    resolved.http_server.port = 0;
    resolved.nats = trogon_nats::NatsConfig::from_url(server.address());
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(CREDENTIAL_MANAGEMENT_ADMIN_TOKEN, "admin-token");

    let error = tokio::time::timeout(
        Duration::from_secs(10),
        serve_with_env(resolved, std::future::ready(()), &env),
    )
    .await
    .unwrap()
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("{OPENBAO_ADDR} must be set when {CREDENTIAL_MANAGEMENT_ADMIN_TOKEN} is enabled")
    );
}

#[test]
fn admin_token_is_absent_when_the_variable_is_not_set() {
    let env = trogon_std::env::InMemoryEnv::new();

    assert!(credential_management_admin_token_from_env(&env).unwrap().is_none());
}

#[test]
fn admin_token_is_read_from_the_environment() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(CREDENTIAL_MANAGEMENT_ADMIN_TOKEN, "admin-token");

    let token = credential_management_admin_token_from_env(&env).unwrap().unwrap();

    assert_eq!(token.as_str(), "admin-token");
}

#[test]
fn empty_admin_token_is_rejected() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(CREDENTIAL_MANAGEMENT_ADMIN_TOKEN, "");

    let error = credential_management_admin_token_from_env(&env).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("{CREDENTIAL_MANAGEMENT_ADMIN_TOKEN} must not be empty")
    );
}

#[test]
fn unreadable_admin_token_is_reported_with_the_variable_name() {
    let error = credential_management_admin_token_from_env(&NonUnicodeEnv).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("failed to read {CREDENTIAL_MANAGEMENT_ADMIN_TOKEN}")
    );
}

#[test]
fn openbao_store_is_built_from_the_environment() {
    let env = management_env("http://openbao.invalid:8200");

    assert!(openbao_secret_store_from_env(&env, CREDENTIAL_MANAGEMENT_ADMIN_TOKEN).is_ok());
}

#[test]
fn openbao_store_requires_an_address() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(OPENBAO_TOKEN, "openbao-token");

    let error = openbao_secret_store_from_env(&env, "management").unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("{OPENBAO_ADDR} must be set when management is enabled")
    );
}

#[test]
fn openbao_store_requires_a_token() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(OPENBAO_ADDR, "http://openbao.invalid:8200");

    let error = openbao_secret_store_from_env(&env, "management").unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("{OPENBAO_TOKEN} must be set when management is enabled")
    );
}

#[test]
fn openbao_store_rejects_an_invalid_address() {
    let env = management_env("not a url");

    let error = openbao_secret_store_from_env(&env, "management").unwrap_err();

    assert_eq!(error.to_string(), "OpenBao secret store configuration");
}

#[test]
fn openbao_store_rejects_a_plaintext_address_by_default() {
    let env = management_env("http://openbao.invalid:8200");
    env.remove(OPENBAO_ALLOW_PLAINTEXT);

    let error = openbao_secret_store_from_env(&env, "management").unwrap_err();

    assert!(matches!(
        error.downcast_ref::<secret_store::openbao_secret_store::OpenBaoSecretStoreConfigError>(),
        Some(secret_store::openbao_secret_store::OpenBaoSecretStoreConfigError::PlaintextAddress)
    ));
}

#[test]
fn openbao_store_accepts_an_https_address_by_default() {
    let env = management_env("https://openbao.invalid:8200");
    env.remove(OPENBAO_ALLOW_PLAINTEXT);

    assert!(openbao_secret_store_from_env(&env, "management").is_ok());
}

#[test]
fn openbao_transport_requires_tls_when_plaintext_is_explicitly_disallowed() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(OPENBAO_ALLOW_PLAINTEXT, "false");

    assert_eq!(openbao_transport_from_env(&env).unwrap(), OpenBaoTransport::TlsRequired);
}

#[test]
fn openbao_transport_rejects_an_unrecognized_flag() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(OPENBAO_ALLOW_PLAINTEXT, "yes");

    let error = openbao_transport_from_env(&env).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("{OPENBAO_ALLOW_PLAINTEXT} must be true or false, got 'yes'")
    );
}

#[test]
fn openbao_transport_reports_an_unreadable_flag() {
    let error = openbao_transport_from_env(&NonUnicodeEnv).unwrap_err();

    assert_eq!(error.to_string(), format!("failed to read {OPENBAO_ALLOW_PLAINTEXT}"));
}

#[tokio::test]
async fn credential_runtime_projection_refresh_logs_the_report_fields() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::INFO);
    let stream = MockJetStreamPublishMessage::new();

    refresh_credential_runtime_projection_from_start(
        &stream,
        &EmptyCredentialEventStore::default(),
        credential_runtime_projection::RuntimeCredentialRegistry::default(),
    )
    .await
    .unwrap();

    let records = events.events();
    let refreshed = records
        .iter()
        .find(|event| event.message() == Some("credential runtime projection refreshed"))
        .unwrap();
    assert_eq!(refreshed.field("scanned_events"), Some("0"));
    assert_eq!(refreshed.field("decoded_events"), Some("0"));
    assert_eq!(refreshed.field("skipped_events"), Some("0"));
    assert_eq!(refreshed.field("changed_credentials"), Some("0"));
    assert_eq!(refreshed.field("applied_credentials"), Some("0"));
    assert_eq!(refreshed.field("projected_integrations"), Some("0"));
    assert_eq!(refreshed.field("checkpoint_loaded_sequence"), Some("0"));
    assert_eq!(refreshed.field("checkpoint_advanced_to"), Some("None"));
}

#[tokio::test]
async fn startup_projection_refresh_over_the_credential_store_logs_the_report() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::INFO);
    let server = JetStreamTestServer::start().await;
    let store = credential::store::open(server.jetstream().await).await.unwrap();

    let (_registry, report) = refresh_credential_runtime_projection(&store).await.unwrap();

    assert_eq!(report.scanned_events(), 0);
    let records = events.events();
    let refreshed = records
        .iter()
        .find(|event| event.message() == Some("credential runtime projection refreshed"))
        .unwrap();
    assert_eq!(refreshed.field("scanned_events"), Some("0"));
}
