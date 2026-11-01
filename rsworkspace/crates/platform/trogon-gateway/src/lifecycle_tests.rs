use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use google_cloud_auth::build_errors;
use tokio::sync::Mutex;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::{ClaimBucketBinding, MockJetStreamPublisher, MockObjectStore};

use crate::credential::commands::domain::{CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
use crate::credential_runtime_projection::{
    RuntimeCredentialRegistry, RuntimeCredentialResolver, RuntimeIntegrationProjection,
};
use crate::secret_store::{MockOpenBaoSecretStore, SecretStorePut, SecretStoreRotate};
use crate::source::google_pubsub::credentials::GoogleCredentialsError;
use crate::source_integration_id::SourceIntegrationId;

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

fn no_runtime_credentials() -> Option<&'static RuntimeCredentialResolver<MockOpenBaoSecretStore>> {
    None
}

async fn telegram_runtime_resolver(
    integration_id: &str,
    secret: Option<&str>,
) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
    let store = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    if let Some(secret) = secret {
        let credential = store
            .put(
                CredentialScope::integration(
                    CredentialOwnerId::new("tenant-1").unwrap(),
                    SourceKind::Telegram,
                    SourceIntegrationId::new(integration_id).unwrap(),
                ),
                CredentialKind::WebhookSecret,
                SecretString::new(secret).unwrap(),
            )
            .await
            .unwrap();
        registry
            .projections()
            .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential, 1).unwrap())
            .await;
    }
    registry.resolver(store)
}

#[tokio::test]
async fn telegram_registration_uses_the_runtime_secret_and_fails_closed_without_one() {
    let resolver = telegram_runtime_resolver("first", Some("runtime-first")).await;
    let registered = Mutex::new(Vec::new());
    register_telegram_webhooks(
        &telegram_integrations(),
        Some(&resolver),
        &mut TelegramWebhookRegistrations::default(),
        || Ok::<_, io::Error>(()),
        async |config, _| {
            registered.lock().await.push(config.webhook_secret.as_str().to_owned());
            Ok(())
        },
    )
    .await;
    assert_eq!(*registered.lock().await, ["runtime-first"]);

    let unprovisioned = telegram_runtime_resolver("other", None).await;
    registered.lock().await.clear();
    register_telegram_webhooks(
        &telegram_integrations(),
        Some(&unprovisioned),
        &mut TelegramWebhookRegistrations::default(),
        || -> Result<(), io::Error> { panic!("an unprovisioned secret needs no client") },
        async |config, _| {
            registered.lock().await.push(config.webhook_secret.as_str().to_owned());
            Ok(())
        },
    )
    .await;
    assert!(registered.lock().await.is_empty());
}

#[tokio::test]
async fn telegram_registration_reports_an_unprovisioned_runtime_secret_once() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let unprovisioned = telegram_runtime_resolver("other", None).await;
    let mut registrations = TelegramWebhookRegistrations::default();
    for _ in 0..2 {
        register_telegram_webhooks(
            &telegram_integrations(),
            Some(&unprovisioned),
            &mut registrations,
            || -> Result<(), io::Error> { panic!("an unprovisioned secret needs no client") },
            async |_, _| -> Result<(), source::telegram::registration::RegistrationError> {
                panic!("an unprovisioned secret is never registered")
            },
        )
        .await;
    }
    let records = events.events();
    let skipped: Vec<_> = records
        .iter()
        .filter(|event| {
            event.message() == Some("Telegram runtime webhook secret is not provisioned, skipping registration")
        })
        .map(|event| event.field("integration"))
        .collect();
    assert_eq!(skipped, [Some("first"), Some("second")]);
    assert!(records.iter().all(
        |event| event.message() != Some("Telegram runtime webhook secret resolution failed, skipping registration")
    ));
}

#[tokio::test]
async fn telegram_registration_follows_a_rotated_runtime_secret_and_skips_an_unchanged_one() {
    let store = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let credential = store
        .put(
            CredentialScope::integration(
                CredentialOwnerId::new("tenant-1").unwrap(),
                SourceKind::Telegram,
                SourceIntegrationId::new("first").unwrap(),
            ),
            CredentialKind::WebhookSecret,
            SecretString::new("runtime-first").unwrap(),
        )
        .await
        .unwrap();
    registry
        .projections()
        .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential.clone(), 1).unwrap())
        .await;
    let resolver = registry.resolver(store.clone());
    let mut registrations = TelegramWebhookRegistrations::default();
    let registered = Mutex::new(Vec::new());
    let register = async |config: &source::telegram::TelegramSourceConfig, _: &()| {
        registered.lock().await.push(config.webhook_secret.as_str().to_owned());
        Ok(())
    };

    register_telegram_webhooks(
        &telegram_integrations(),
        Some(&resolver),
        &mut registrations,
        || Ok::<_, io::Error>(()),
        register,
    )
    .await;
    register_telegram_webhooks(
        &telegram_integrations(),
        Some(&resolver),
        &mut registrations,
        || -> Result<(), io::Error> { panic!("an unchanged secret needs no client") },
        register,
    )
    .await;
    assert_eq!(*registered.lock().await, ["runtime-first"]);

    let rotated = store
        .rotate(&credential, SecretString::new("runtime-rotated").unwrap())
        .await
        .unwrap();
    registry
        .projections()
        .upsert(RuntimeIntegrationProjection::active_from_credential_ref(rotated.clone(), 2).unwrap())
        .await;
    register_telegram_webhooks(
        &telegram_integrations(),
        Some(&resolver),
        &mut registrations,
        || Ok::<_, io::Error>(()),
        register,
    )
    .await;
    assert_eq!(*registered.lock().await, ["runtime-first", "runtime-rotated"]);
    assert_eq!(
        registrations
            .registered_credentials
            .get(&source_integration_id::SourceIntegrationId::new("first").unwrap()),
        Some(rotated)
    );
}

#[tokio::test]
async fn telegram_registration_reuses_client_and_continues_after_one_rejection() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::ERROR);
    let clients = AtomicUsize::new(0);
    let registered = Mutex::new(Vec::new());
    register_telegram_webhooks(
        &telegram_integrations(),
        no_runtime_credentials(),
        &mut TelegramWebhookRegistrations::default(),
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
        no_runtime_credentials(),
        &mut TelegramWebhookRegistrations::default(),
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
        no_runtime_credentials(),
        &mut TelegramWebhookRegistrations::default(),
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
    trogon_std::tls::install_default_crypto_provider();
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

fn discord_without_static_bot_token() -> config::ResolvedConfig {
    configuration(
        r#"
[sources.discord]
gateway_intents = "guilds,guild_messages"
"#,
    )
}

#[tokio::test(start_paused = true)]
async fn discord_runtime_bot_token_retry_stops_on_shutdown() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let resolved = discord_without_static_bot_token();
    let (shutdown, signal) = tokio::sync::oneshot::channel();
    let mut tasks = JoinSet::new();
    spawn_discord_gateway_with_runtime_credentials(
        &mut tasks,
        resolved.discord.as_ref(),
        RuntimeCredentialRegistry::default().resolver(MockOpenBaoSecretStore::default()),
        publisher(),
        async { signal.await.unwrap() },
        async |_, _, _| unreachable!("the gateway must not start without a bot token"),
    );

    tokio::time::sleep(Duration::from_secs(10)).await;
    shutdown.send(()).unwrap();
    let (name, result) = tokio::time::timeout(Duration::from_secs(1), tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(name, "discord-gateway");
    result.unwrap();
    let failed_attempts = events
        .events()
        .iter()
        .filter(|event| event.message() == Some("Discord runtime bot token could not be resolved"))
        .count();
    assert!(failed_attempts > 1);
}

async fn put_discord_bot_token(
    store: &MockOpenBaoSecretStore,
    registry: &RuntimeCredentialRegistry,
    token: &str,
) -> crate::credential::commands::domain::CredentialRef {
    let credential = store
        .put(
            CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord),
            CredentialKind::BotToken,
            SecretString::new(token).unwrap(),
        )
        .await
        .unwrap();
    registry
        .projections()
        .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential.clone(), 1).unwrap())
        .await;
    credential
}

#[tokio::test(start_paused = true)]
async fn discord_runtime_gateway_starts_once_the_bot_token_projection_catches_up_and_restarts_an_ended_shard() {
    trogon_std::tls::install_default_crypto_provider();
    let resolved = discord_without_static_bot_token();
    let store = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let (shard_tokens, mut observed_shard_tokens) = tokio::sync::mpsc::unbounded_channel();
    let (shutdown, signal) = tokio::sync::oneshot::channel();
    let mut tasks = JoinSet::new();
    spawn_discord_gateway_with_runtime_credentials(
        &mut tasks,
        resolved.discord.as_ref(),
        registry.resolver(store.clone()),
        publisher(),
        async { signal.await.unwrap() },
        move |_, _, shard| {
            let shard_tokens = shard_tokens.clone();
            async move {
                shard_tokens.send(shard.config().token().to_owned()).unwrap();
            }
        },
    );

    tokio::time::sleep(Duration::from_millis(1500)).await;
    put_discord_bot_token(&store, &registry, "runtime-token").await;
    for _ in 0..2 {
        let token = tokio::time::timeout(Duration::from_secs(60), observed_shard_tokens.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token, "Bot runtime-token");
    }

    shutdown.send(()).unwrap();
    let (name, result) = tokio::time::timeout(Duration::from_secs(1), tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(name, "discord-gateway");
    result.unwrap();
}

#[tokio::test(start_paused = true)]
async fn discord_runtime_gateway_restarts_the_shard_with_a_rotated_bot_token() {
    trogon_std::tls::install_default_crypto_provider();
    let resolved = discord_without_static_bot_token();
    let store = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let credential = put_discord_bot_token(&store, &registry, "runtime-token").await;
    let (shard_tokens, mut observed_shard_tokens) = tokio::sync::mpsc::unbounded_channel();
    let mut tasks = JoinSet::new();
    spawn_discord_gateway_with_runtime_credentials(
        &mut tasks,
        resolved.discord.as_ref(),
        registry.resolver(store.clone()),
        publisher(),
        std::future::pending(),
        move |_, _, shard| {
            let shard_tokens = shard_tokens.clone();
            async move {
                shard_tokens.send(shard.config().token().to_owned()).unwrap();
                std::future::pending::<()>().await;
            }
        },
    );
    assert_eq!(observed_shard_tokens.recv().await.unwrap(), "Bot runtime-token");

    tokio::time::sleep(CREDENTIAL_RUNTIME_PROJECTION_WORKER_INTERVAL * 2).await;
    assert!(observed_shard_tokens.try_recv().is_err());

    let rotated = store
        .rotate(&credential, SecretString::new("runtime-rotated").unwrap())
        .await
        .unwrap();
    registry
        .projections()
        .upsert(RuntimeIntegrationProjection::active_from_credential_ref(rotated.clone(), 2).unwrap())
        .await;
    let token = tokio::time::timeout(Duration::from_secs(60), observed_shard_tokens.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(token, "Bot runtime-rotated");
    assert!(observed_shard_tokens.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn discord_runtime_gateway_resets_the_restart_delay_after_a_healthy_shard_run() {
    trogon_std::tls::install_default_crypto_provider();
    let resolved = discord_without_static_bot_token();
    let store = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    put_discord_bot_token(&store, &registry, "runtime-token").await;
    let backoff = crate::supervision::RestartBackoff::new(Duration::from_secs(1), Duration::from_secs(4));
    let healthy_run = Duration::from_secs(10);
    let runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (starts, mut observed_starts) = tokio::sync::mpsc::unbounded_channel();
    let supervisor = tokio::spawn(supervise_discord_runtime_gateway(
        registry.resolver(store.clone()),
        publisher(),
        resolved.discord.clone().unwrap(),
        move |_, _, _| {
            let run = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            starts.send(tokio::time::Instant::now()).unwrap();
            async move {
                if run == 3 {
                    tokio::time::sleep(healthy_run).await;
                }
            }
        },
        backoff,
        Duration::from_secs(3600),
    ));

    let mut started = Vec::new();
    for _ in 0..5 {
        started.push(
            tokio::time::timeout(Duration::from_secs(60), observed_starts.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    supervisor.abort();

    let gaps: Vec<Duration> = started.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(
        gaps,
        vec![
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4),
            healthy_run + backoff.initial(),
        ]
    );
}

fn slack_integrations() -> Vec<config::SourceIntegration<source::slack::SlackConfig>> {
    configuration(
        r#"
[sources.slack.integrations.socket.socket_mode]
intended_use = "development"
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
        AdvancedMockNatsClient::new(),
        std::future::pending(),
        async |_, _, config, _| {
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
        AdvancedMockNatsClient::new(),
        async { signal.await.unwrap() }.shared(),
        move |_, _, _, _| {
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

fn google_pubsub_integrations() -> Vec<config::SourceIntegration<source::google_pubsub::GooglePubSubConfig>> {
    configuration(
        r#"
[sources.google_pubsub.integrations.orders.streaming_pull]
subscription = "projects/p/subscriptions/orders"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
[sources.google_pubsub.integrations.billing.streaming_pull]
subscription = "projects/p/subscriptions/billing"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#,
    )
    .google_pubsub
}

#[tokio::test]
async fn google_pubsub_spawns_one_runner_per_integration_with_its_own_config() {
    let google_pubsub_integrations = google_pubsub_integrations();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut tasks = JoinSet::new();
    let runner_seen = seen.clone();
    spawn_google_pubsub_streaming_pulls(
        &mut tasks,
        prepare_google_pubsub_sources(&google_pubsub_integrations).unwrap(),
        publisher(),
        std::future::pending::<()>().shared(),
        move |_, config, _, _| {
            let seen = runner_seen.clone();
            async move {
                let source::google_pubsub::config::GooglePubSubTransportConfig::StreamingPull(streaming_pull) =
                    config.transport;
                seen.lock().unwrap().push(streaming_pull.subscription.to_string());
            }
        },
    );
    assert_eq!(tasks.len(), 2);
    while let Some(result) = tasks.join_next().await {
        let (name, result) = result.unwrap();
        assert_eq!(name, "google-pubsub-streaming-pull");
        result.unwrap();
    }
    let mut subscriptions = seen.lock().unwrap().clone();
    subscriptions.sort();
    assert_eq!(
        subscriptions,
        vec![
            "projects/p/subscriptions/billing".to_string(),
            "projects/p/subscriptions/orders".to_string()
        ]
    );
}

#[tokio::test]
async fn google_pubsub_preparation_names_the_integration_whose_credentials_fail_to_build() {
    let integrations = configuration(
        r#"
[sources.google_pubsub.integrations.orders.streaming_pull]
subscription = "projects/p/subscriptions/orders"
credentials = '{"type":"service_account","client_email":"leaky-marker@example.invalid"}'
"#,
    )
    .google_pubsub;

    let error = prepare_google_pubsub_sources(&integrations).err().unwrap();

    assert!(error.to_string().contains("google_pubsub/orders"));
    assert!(matches!(
        error.downcast_ref::<GoogleCredentialsError>(),
        Some(GoogleCredentialsError::MalformedServiceAccount(_))
    ));
    assert!(
        error
            .chain()
            .any(|cause| cause.downcast_ref::<build_errors::Error>().is_some())
    );
}

#[tokio::test]
async fn google_pubsub_preparation_reports_the_first_failing_integration_among_valid_ones() {
    let integrations = configuration(
        r#"
[sources.google_pubsub.integrations.billing.streaming_pull]
subscription = "projects/p/subscriptions/billing"
credentials = '{"type":"external_account","audience":"a"}'
[sources.google_pubsub.integrations.orders.streaming_pull]
subscription = "projects/p/subscriptions/orders"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#,
    )
    .google_pubsub;

    let error = prepare_google_pubsub_sources(&integrations).err().unwrap();

    assert!(error.to_string().contains("google_pubsub/billing"));
    assert!(matches!(
        error.downcast_ref::<GoogleCredentialsError>(),
        Some(GoogleCredentialsError::MalformedExternalAccount(_))
    ));
}

#[tokio::test]
async fn google_pubsub_without_integrations_spawns_nothing() {
    let mut tasks = JoinSet::new();
    spawn_google_pubsub_streaming_pulls(
        &mut tasks,
        Vec::new(),
        publisher(),
        std::future::pending::<()>().shared(),
        async |_, _, _, _| unreachable!("no integration is configured"),
    );
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn google_pubsub_runners_receive_the_shared_shutdown_signal() {
    let google_pubsub_integrations = google_pubsub_integrations();
    let (shutdown, signal) = tokio::sync::oneshot::channel();
    let mut tasks = JoinSet::new();
    spawn_google_pubsub_streaming_pulls(
        &mut tasks,
        prepare_google_pubsub_sources(&google_pubsub_integrations).unwrap(),
        publisher(),
        async { signal.await.unwrap() }.shared(),
        async |_, _, _, shutdown| shutdown.await,
    );
    assert_eq!(tasks.len(), 2);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), tasks.join_next())
            .await
            .is_err()
    );
    shutdown.send(()).unwrap();
    while let Some(result) = tokio::time::timeout(Duration::from_secs(5), tasks.join_next())
        .await
        .unwrap()
    {
        let (name, result) = result.unwrap();
        assert_eq!(name, "google-pubsub-streaming-pull");
        result.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn slack_runner_that_panics_is_restarted_instead_of_ending_the_source() {
    let runs = Arc::new(AtomicUsize::new(0));
    let (started, mut observed) = tokio::sync::mpsc::channel(1);
    let (shutdown, signal) = tokio::sync::oneshot::channel();
    let mut tasks = JoinSet::new();
    let runner_runs = runs.clone();
    spawn_slack_socket_modes(
        &mut tasks,
        &slack_integrations(),
        publisher(),
        AdvancedMockNatsClient::new(),
        async { signal.await.unwrap() }.shared(),
        move |_, _, _, _| {
            let started = started.clone();
            let runs = runner_runs.clone();
            async move {
                if runs.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("socket mode runner crashed");
                }
                started.send(()).await.unwrap();
                std::future::pending().await
            }
        },
    );
    tokio::time::timeout(Duration::from_secs(5), observed.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    shutdown.send(()).unwrap();
    let (name, result) = tasks.join_next().await.unwrap().unwrap();
    assert_eq!(name, "slack-socket-mode");
    result.unwrap();
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

#[tokio::test]
async fn discord_without_a_static_bot_token_warns_and_never_starts_the_gateway() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let resolved = discord_without_static_bot_token();
    let mut tasks = JoinSet::new();
    spawn_discord_gateway(&mut tasks, resolved.discord.as_ref(), publisher(), async |_, _, _| {
        unreachable!("the gateway must not start without a bot token")
    });

    let (name, result) = tasks.join_next().await.unwrap().unwrap();

    assert_eq!(name, "discord-gateway");
    result.unwrap();
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.message() == Some("Discord gateway configured without a static bot token"))
    );
}

#[tokio::test]
async fn discord_runtime_gateway_spawns_nothing_without_a_discord_source() {
    let mut tasks = JoinSet::new();
    spawn_discord_gateway_with_runtime_credentials(
        &mut tasks,
        None,
        RuntimeCredentialRegistry::default().resolver(MockOpenBaoSecretStore::default()),
        publisher(),
        std::future::pending(),
        async |_, _, _| unreachable!("there is no discord source"),
    );

    assert!(tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn telegram_registration_worker_keeps_polling_until_it_is_dropped() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::WARN);
    let worker = run_telegram_webhook_registration_worker(
        telegram_integrations(),
        telegram_runtime_resolver("other", None).await,
        TelegramWebhookRegistrations::default(),
        Duration::from_secs(10),
    );

    assert!(tokio::time::timeout(Duration::from_secs(35), worker).await.is_err());

    let skipped = events
        .events()
        .iter()
        .filter(|event| {
            event.message() == Some("Telegram runtime webhook secret is not provisioned, skipping registration")
        })
        .count();
    assert_eq!(skipped, 2);
}

#[tokio::test]
async fn telegram_registration_skips_an_integration_whose_runtime_secret_cannot_be_read() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::ERROR);
    let writer = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let credential = writer
        .put(
            CredentialScope::integration(
                CredentialOwnerId::new("tenant-1").unwrap(),
                SourceKind::Telegram,
                SourceIntegrationId::new("first").unwrap(),
            ),
            CredentialKind::WebhookSecret,
            SecretString::new("runtime-first").unwrap(),
        )
        .await
        .unwrap();
    registry
        .projections()
        .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential, 1).unwrap())
        .await;
    let resolver = registry.resolver(MockOpenBaoSecretStore::default());
    let attempts = AtomicUsize::new(0);

    register_telegram_webhooks(
        &telegram_integrations(),
        Some(&resolver),
        &mut TelegramWebhookRegistrations::default(),
        || Ok::<_, io::Error>(()),
        async |_, _| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;

    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert!(events.events().iter().any(
        |event| event.message() == Some("Telegram runtime webhook secret resolution failed, skipping registration")
    ));
}

fn runtime_credential_env(flags: &[&str]) -> trogon_std::env::InMemoryEnv {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(OPENBAO_ADDR, "http://127.0.0.1:1");
    env.set(OPENBAO_TOKEN, "openbao-token");
    env.set(OPENBAO_ALLOW_PLAINTEXT, "true");
    for flag in flags {
        env.set(*flag, "true");
    }
    env
}

type OpenBaoMounts = source_plugin::RuntimeCredentialMounts<secret_store::openbao_secret_store::OpenBaoSecretStore>;

fn mounted_sources(mounts: &OpenBaoMounts) -> [bool; 11] {
    [
        mounts.github.is_some(),
        mounts.gitlab.is_some(),
        mounts.incidentio.is_some(),
        mounts.linear.is_some(),
        mounts.microsoft_graph.is_some(),
        mounts.notion.is_some(),
        mounts.plaid.is_some(),
        mounts.sentry.is_some(),
        mounts.slack.is_some(),
        mounts.telegram.is_some(),
        mounts.twitter.is_some(),
    ]
}

#[test]
fn each_runtime_credential_flag_mounts_only_its_own_source() {
    let flags = [
        ENABLE_RUNTIME_GITHUB_CREDENTIALS,
        ENABLE_RUNTIME_GITLAB_CREDENTIALS,
        ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS,
        ENABLE_RUNTIME_LINEAR_CREDENTIALS,
        ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS,
        ENABLE_RUNTIME_NOTION_CREDENTIALS,
        ENABLE_RUNTIME_PLAID_CREDENTIALS,
        ENABLE_RUNTIME_SENTRY_CREDENTIALS,
        ENABLE_RUNTIME_SLACK_CREDENTIALS,
        ENABLE_RUNTIME_TELEGRAM_CREDENTIALS,
        ENABLE_RUNTIME_TWITTER_CREDENTIALS,
    ];
    let registry = RuntimeCredentialRegistry::default();

    for (index, flag) in flags.iter().enumerate() {
        let mounts = source_runtime_credentials_from_env(&runtime_credential_env(&[*flag]), &registry)
            .unwrap()
            .unwrap();

        let mut expected = [false; 11];
        expected[index] = true;
        assert_eq!(mounted_sources(&mounts), expected, "{flag}");
    }
}

#[test]
fn several_runtime_credential_flags_mount_every_flagged_source_and_log_each() {
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _guard = events.install(trogon_std::log_capture::LevelFilter::INFO);
    let flags = [
        ENABLE_RUNTIME_GITHUB_CREDENTIALS,
        ENABLE_RUNTIME_GITLAB_CREDENTIALS,
        ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS,
        ENABLE_RUNTIME_LINEAR_CREDENTIALS,
        ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS,
        ENABLE_RUNTIME_NOTION_CREDENTIALS,
        ENABLE_RUNTIME_PLAID_CREDENTIALS,
        ENABLE_RUNTIME_SENTRY_CREDENTIALS,
        ENABLE_RUNTIME_SLACK_CREDENTIALS,
        ENABLE_RUNTIME_TELEGRAM_CREDENTIALS,
        ENABLE_RUNTIME_TWITTER_CREDENTIALS,
    ];

    let mounts =
        source_runtime_credentials_from_env(&runtime_credential_env(&flags), &RuntimeCredentialRegistry::default())
            .unwrap()
            .unwrap();

    assert_eq!(mounted_sources(&mounts), [true; 11]);
    let enabled = events
        .events()
        .iter()
        .filter(|event| {
            event
                .message()
                .is_some_and(|message| message.ends_with("webhook runtime credentials enabled"))
        })
        .count();
    assert_eq!(enabled, 11);
}

#[test]
fn no_runtime_credential_flag_mounts_nothing_and_needs_no_openbao() {
    let env = trogon_std::env::InMemoryEnv::new();

    assert!(
        source_runtime_credentials_from_env(&env, &RuntimeCredentialRegistry::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_runtime_credential_flag_requires_the_openbao_settings() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set(ENABLE_RUNTIME_GITHUB_CREDENTIALS, "true");

    let error = source_runtime_credentials_from_env(&env, &RuntimeCredentialRegistry::default())
        .err()
        .unwrap();

    assert_eq!(
        error.to_string(),
        format!("{OPENBAO_ADDR} must be set when {ENABLE_RUNTIME_GITHUB_CREDENTIALS} is enabled")
    );
}

#[test]
fn discord_runtime_credentials_follow_their_flag() {
    let registry = RuntimeCredentialRegistry::default();

    assert!(
        discord_runtime_credentials_from_env(&trogon_std::env::InMemoryEnv::new(), &registry)
            .unwrap()
            .is_none()
    );
    assert!(
        discord_runtime_credentials_from_env(
            &runtime_credential_env(&[ENABLE_RUNTIME_DISCORD_CREDENTIALS]),
            &registry
        )
        .unwrap()
        .is_some()
    );
    let missing_settings = trogon_std::env::InMemoryEnv::new();
    missing_settings.set(ENABLE_RUNTIME_DISCORD_CREDENTIALS, "true");
    assert!(discord_runtime_credentials_from_env(&missing_settings, &registry).is_err());
}
