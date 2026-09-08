use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use trogon_nats::jetstream::{ClaimBucketBinding, MockJetStreamPublisher, MockObjectStore};

use super::*;

#[test]
fn nats_connection_diagnostics_report_the_claim_threshold_in_log_facade() {
    let Some(logs) = trogon_std::log_capture::CapturedLogs::isolated() else {
        return;
    };
    log_nats_connection(16384, MaxPayload::from_server_limit(16384));
    let records = logs.records();
    let connected = records
        .iter()
        .find(|record| record.target == "trogon_gateway" && record.message.contains("NATS connected"))
        .unwrap();
    assert_eq!(connected.level, trogon_std::log_capture::LogLevel::Info);
    assert!(connected.message.contains("server_max_payload_bytes=16384"));
    assert!(connected.message.contains("claim_check_threshold_bytes=8192"));
}

#[test]
fn nats_connection_diagnostics_preserve_separate_payload_fields() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::INFO);
    log_nats_connection(16384, MaxPayload::from_server_limit(16384));
    let records = events.events();
    let connected = records
        .iter()
        .find(|event| event.message() == Some("NATS connected"))
        .unwrap();
    assert_eq!(connected.field("server_max_payload_bytes"), Some("16384"));
    assert_eq!(connected.field("claim_check_threshold_bytes"), Some("8192"));
}

fn configuration(toml: &str) -> config::ResolvedConfig {
    let mut file = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    file.write_all(toml.as_bytes()).unwrap();
    config::load(Some(file.path())).unwrap()
}

fn publisher() -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        MockJetStreamPublisher::new(),
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn telegram_integrations() -> Vec<config::SourceIntegration<source::telegram::TelegramSourceConfig>> {
    configuration(
        r#"
[sources.telegram.integrations.first.webhook]
webhook_secret = "secret-first"
webhook_registration_mode = "startup"
bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ"
public_webhook_url = "https://example.com/sources/telegram/first/webhook"
[sources.telegram.integrations.second.webhook]
webhook_secret = "secret-second"
webhook_registration_mode = "startup"
bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ"
public_webhook_url = "https://example.com/sources/telegram/second/webhook"
[sources.telegram.integrations.passive.webhook]
webhook_secret = "secret-passive"
"#,
    )
    .telegram
}

#[tokio::test]
async fn telegram_registration_reuses_client_and_continues_after_one_rejection() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::ERROR);
    let clients = AtomicUsize::new(0);
    let registered = Mutex::new(Vec::new());
    register_telegram_webhooks(
        &telegram_integrations(),
        || {
            clients.fetch_add(1, Ordering::SeqCst);
            Ok::<_, io::Error>(7)
        },
        async |config, client| {
            assert_eq!(*client, 7);
            let secret = config.webhook_secret.as_str();
            registered.lock().await.push(secret.to_owned());
            if secret == "secret-first" {
                Err(source::telegram::registration::RegistrationError::Rejected {
                    status: reqwest::StatusCode::BAD_REQUEST,
                    description: "fixture rejection".to_owned(),
                })
            } else {
                Ok(())
            }
        },
    )
    .await;
    assert_eq!(clients.load(Ordering::SeqCst), 1);
    assert_eq!(*registered.lock().await, ["secret-first", "secret-second"]);
    let records = events.events();
    let rejection = records
        .iter()
        .find(|event| event.message() == Some("Telegram webhook registration failed"))
        .unwrap();
    assert_eq!(rejection.field("source"), Some("telegram"));
    assert_eq!(rejection.field("integration"), Some("first"));
    assert_eq!(
        rejection.field("error"),
        Some("Telegram webhook registration rejected with 400 Bad Request: fixture rejection")
    );
}

#[tokio::test]
async fn failed_telegram_client_skips_registration_and_empty_sources_skip_client_creation() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::ERROR);
    let attempts = AtomicUsize::new(0);
    register_telegram_webhooks(
        &telegram_integrations(),
        || Err::<(), _>(io::Error::from(io::ErrorKind::PermissionDenied)),
        async |_, _| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    register_telegram_webhooks(
        &[],
        || -> Result<(), io::Error> { panic!("client is unnecessary without registrations") },
        async |_, _| -> Result<(), source::telegram::registration::RegistrationError> {
            panic!("no registration configured")
        },
    )
    .await;
    let records = events.events();
    let failure = records
        .iter()
        .find(|event| event.message() == Some("Telegram webhook registration HTTP client initialization failed"))
        .unwrap();
    assert_eq!(failure.field("source"), Some("telegram"));
    assert_eq!(
        failure.field("error"),
        Some(io::Error::from(io::ErrorKind::PermissionDenied).to_string().as_str())
    );
}

#[tokio::test]
async fn discord_task_receives_configured_shard_and_reports_normal_completion() {
    trogon_std::tls::install_default_crypto_provider().unwrap();
    let resolved = configuration(
        r#"
[sources.discord]
bot_token = "fixture-token"
gateway_intents = "guilds,guild_messages"
"#,
    );
    let mut tasks = JoinSet::new();
    spawn_discord_gateway(
        &mut tasks,
        resolved.discord.as_ref(),
        publisher(),
        async |_, config, shard| {
            assert_eq!(shard.id(), twilight_gateway::ShardId::ONE);
            assert_eq!(shard.config().intents(), config.intents);
            assert_eq!(shard.config().token(), "Bot fixture-token");
            assert_eq!(config.subject_prefix.as_str(), "discord");
        },
    );
    let (name, result) = tasks.join_next().await.unwrap().unwrap();
    assert_eq!(name, "discord-gateway");
    result.unwrap();
    assert!(tasks.is_empty());
}

fn slack_integrations() -> Vec<config::SourceIntegration<source::slack::SlackConfig>> {
    configuration(
        r#"
[sources.slack.integrations.socket.socket_mode]
app_token = "xapp-fixture-token"
[sources.slack.integrations.webhook.webhook]
signing_secret = "fixture-secret"
"#,
    )
    .slack
}

#[tokio::test]
async fn slack_task_reports_runner_error_with_context_and_skips_webhook_integrations() {
    let mut tasks = JoinSet::new();
    spawn_slack_socket_modes(
        &mut tasks,
        &slack_integrations(),
        publisher(),
        std::future::pending(),
        async |_, config, _| {
            assert_eq!(config.socket_mode().unwrap().app_token.as_str(), "xapp-fixture-token");
            Err(source::slack::socket_mode::SocketModeError::MissingSocketModeConfig)
        },
    );
    assert_eq!(tasks.len(), 1);
    let (name, result) = tasks.join_next().await.unwrap().unwrap();
    assert_eq!(name, "slack-socket-mode");
    let error = result.unwrap_err();
    assert_eq!(error.to_string(), "slack socket mode");
    assert!(matches!(
        error.downcast_ref::<source::slack::socket_mode::SocketModeError>(),
        Some(source::slack::socket_mode::SocketModeError::MissingSocketModeConfig)
    ));
}

struct DroppedRunner(Arc<AtomicUsize>);

impl Drop for DroppedRunner {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn slack_shutdown_drops_a_started_pending_runner() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let (started, mut observed) = tokio::sync::mpsc::channel(1);
    let (shutdown, signal) = tokio::sync::oneshot::channel();
    let mut tasks = JoinSet::new();
    let runner_dropped = dropped.clone();
    spawn_slack_socket_modes(
        &mut tasks,
        &slack_integrations(),
        publisher(),
        async { signal.await.unwrap() }.shared(),
        move |_, _, _| {
            let started = started.clone();
            let dropped = runner_dropped.clone();
            async move {
                let _guard = DroppedRunner(dropped);
                started.send(()).await.unwrap();
                std::future::pending().await
            }
        },
    );
    tokio::time::timeout(Duration::from_secs(5), observed.recv())
        .await
        .unwrap()
        .unwrap();
    shutdown.send(()).unwrap();
    let (name, result) = tokio::time::timeout(Duration::from_secs(5), tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(name, "slack-socket-mode");
    result.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn telemetry_shutdown_failure_does_not_reclassify_a_successful_source() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::ERROR);
    let mut tasks = JoinSet::new();
    tasks.spawn(async { ("healthy", Ok(())) });
    let shutdowns = AtomicUsize::new(0);
    wait_for_sources_with_shutdown(tasks, || {
        shutdowns.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    })
    .await
    .unwrap();
    assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    let records = events.events();
    let failure = records
        .iter()
        .find(|event| event.message() == Some("OpenTelemetry shutdown failed"))
        .unwrap();
    assert_eq!(
        failure.field("error"),
        Some(io::Error::from(io::ErrorKind::BrokenPipe).to_string().as_str())
    );
}
