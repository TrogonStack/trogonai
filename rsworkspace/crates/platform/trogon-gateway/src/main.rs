#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod cli;
mod config;
mod constants;
mod credential;
mod credential_lifecycle;
mod credential_management;
mod credential_runtime_projection;
mod http;
mod secret_store;
mod source;
mod source_integration_id;
mod source_plugin;
mod source_status;
mod streams;
mod supervision;

use std::error::Error;
use std::io::Write;
use std::net::SocketAddr;

use crate::constants::{
    CLAIM_CHECK_TTL_GRACE, CREDENTIAL_MANAGEMENT_ADMIN_TOKEN, CREDENTIAL_RUNTIME_PROJECTION_WORKER_INTERVAL,
    CREDENTIAL_WORKER_INTERVAL, NATS_CONNECT_TIMEOUT, NATS_SERVER_INFO_POLL_INTERVAL, OPENBAO_ADDR,
    OPENBAO_ALLOW_PLAINTEXT, OPENBAO_TOKEN, SLACK_RESTART_BACKOFF,
};
use crate::secret_store::openbao_secret_store::OpenBaoTransport;
use anyhow::Context;
use futures_util::FutureExt;
use tokio::task::JoinSet;
use tracing::{error, info};
use trogon_decider_runtime::StreamRead;
use trogon_nats::client::RequestClient;
use trogon_nats::jetstream::{
    ClaimBucket, ClaimCheckPublisher, ClaimRetention, JetStreamGetRawMessage, JetStreamGetStreamInfo,
    JetStreamPublisher, MaxPayload, NatsJetStreamClient, NatsObjectStore, ObjectStorePut,
};
use trogon_nats::{connect, wait_for_server_info};
use trogon_std::SecretString;
use trogon_std::args::{CliArgs, ParseArgs};
use trogon_std::env::{ReadEnv, SystemEnv};
use trogon_std::fs::SystemFs;

type SourceResult = (&'static str, anyhow::Result<()>);

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() -> anyhow::Result<()> {
    trogon_std::tls::install_default_crypto_provider()?;

    let cli = CliArgs::<cli::Cli>::new().parse_args();
    let resolved = config::load_with_overrides(cli.runtime.config.as_deref(), &cli.runtime.nats)?;

    match cli.command {
        cli::Command::Serve => serve(resolved, trogon_std::signal::shutdown_signal()).await,
        cli::Command::Source { source } => match source {
            cli::SourceCommand::Notion { command } => match command {
                cli::NotionCommand::VerificationToken { integration, watch } => {
                    let integration = source_integration_id::SourceIntegrationId::new(&integration)?;
                    let nats = connect(&resolved.nats, NATS_CONNECT_TIMEOUT).await?;
                    let js_context = async_nats::jetstream::new(nats.clone());
                    let mut stdout = std::io::stdout();
                    notion_verification_token(&resolved, &integration, watch, &nats, &js_context, &mut stdout).await
                }
            },
        },
    }
}

async fn serve(
    resolved: config::ResolvedConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    serve_with_env(resolved, shutdown, &SystemEnv).await
}

async fn serve_with_env(
    resolved: config::ResolvedConfig,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    env: &impl ReadEnv,
) -> anyhow::Result<()> {
    if !resolved.has_any_source() {
        anyhow::bail!("no sources configured — provide a config file with at least one source");
    }

    trogon_telemetry::init_logger(trogon_telemetry::ServiceName::TrogonGateway, [], &SystemEnv, &SystemFs);

    info!("trogon-gateway starting");

    let google_pubsub_sources = prepare_google_pubsub_sources(&resolved.google_pubsub)?;

    let nats = connect(&resolved.nats, NATS_CONNECT_TIMEOUT).await?;
    let server_max_payload = wait_for_server_info(&nats, NATS_CONNECT_TIMEOUT, NATS_SERVER_INFO_POLL_INTERVAL)
        .await?
        .max_payload;
    let max_payload = MaxPayload::from_server_limit(server_max_payload);
    log_nats_connection(server_max_payload, max_payload);
    let js_context = async_nats::jetstream::new(nats.clone());
    // Size the claim bucket TTL from the longest stream it serves so a message
    // still on the stream can always resolve its claim. `has_any_source` is
    // enforced above, so `max_stream_max_age` is always `Some` here; the fallback
    // never expires rather than risk reclaiming a live claim.
    let claim_retention = resolved
        .max_stream_max_age()
        .map(|stream_max_age| ClaimRetention::tracking(stream_max_age, CLAIM_CHECK_TTL_GRACE))
        .unwrap_or(ClaimRetention::EventSourced);
    let claim_binding =
        NatsObjectStore::provision_claim_bucket(&js_context, ClaimBucket::default(), claim_retention).await?;
    let client = NatsJetStreamClient::new(js_context);

    streams::provision(&client, &resolved).await?;
    let credential_store = credential::store::open(client.context().clone()).await?;
    info!(
        stream = credential::constants::CREDENTIAL_STREAM,
        snapshot_bucket = credential::constants::CREDENTIAL_SNAPSHOT_BUCKET,
        "credential event store opened"
    );
    let (runtime_credentials, _credential_projection_refresh_report) =
        refresh_credential_runtime_projection(&credential_store).await?;
    register_telegram_webhooks(
        &resolved.telegram,
        crate::source::telegram::registration::registration_http_client,
        crate::source::telegram::registration::register_webhook,
    )
    .await;

    let port = resolved.http_server.port;
    let mut join_set: JoinSet<SourceResult> = JoinSet::new();
    let shutdown = shutdown.shared();

    let publisher = ClaimCheckPublisher::new(client.clone(), claim_binding, nats.clone());
    {
        let projection_runtime_credentials = runtime_credentials.clone();
        let projection_event_stream = credential_store.events_stream().clone();
        let projection_event_store = credential_store.clone();
        let shutdown = shutdown.clone();
        join_set.spawn(async move {
            credential_runtime_projection::run_tracked_refresh_worker(
                projection_runtime_credentials,
                projection_event_stream,
                projection_event_store,
                CREDENTIAL_RUNTIME_PROJECTION_WORKER_INTERVAL,
                shutdown,
            )
            .await;
            ("credential-runtime-projection-worker", Ok(()))
        });
        info!("credential runtime projection refresh worker spawned");
    }

    spawn_discord_gateway(
        &mut join_set,
        resolved.discord.as_ref(),
        publisher.clone(),
        crate::source::discord::gateway_runner::run,
    );

    spawn_slack_socket_modes(
        &mut join_set,
        &resolved.slack,
        publisher.clone(),
        nats.clone(),
        shutdown.clone(),
        crate::source::slack::socket_mode::run,
    );

    spawn_google_pubsub_streaming_pulls(
        &mut join_set,
        google_pubsub_sources,
        publisher.clone(),
        shutdown.clone(),
        crate::source::google_pubsub::streaming_pull::run,
    );

    let app = http::mount_sources(resolved, publisher, &nats);
    let app = match credential_management_admin_token_from_env(env)? {
        Some(admin_token) => {
            let store = openbao_secret_store_from_env(env, CREDENTIAL_MANAGEMENT_ADMIN_TOKEN)?;
            let idempotency = credential_management::idempotency::open(client.context().clone()).await?;
            let worker_checkpoints =
                credential_lifecycle::recovery_worker::open_checkpoint_store(client.context()).await?;
            let recovery_status_checkpoints = worker_checkpoints.clone();
            let handler = credential_lifecycle::handler::CredentialRuntimeHandler::new(
                credential_store.clone(),
                store,
                runtime_credentials.clone(),
            );
            let worker_event_stream = credential_store.events_stream().clone();
            let worker_event_store = credential_store.clone();
            let worker_handler = handler.clone();
            let worker_shutdown = shutdown.clone();
            join_set.spawn(async move {
                credential_lifecycle::recovery_worker::run(
                    worker_event_stream,
                    worker_event_store,
                    worker_handler,
                    worker_checkpoints,
                    CREDENTIAL_WORKER_INTERVAL,
                    worker_shutdown,
                )
                .await;
                ("credential-worker", Ok(()))
            });
            info!("credential recovery worker spawned");
            info!("credential management API enabled");
            let credential_management_routes =
                credential_management::router(handler, admin_token.clone(), idempotency).merge(
                    credential_management::recovery_status_router(admin_token, recovery_status_checkpoints),
                );
            app.nest("/-/credentials", credential_management_routes)
        }
        None => app,
    };
    let app = trogon_std::telemetry::http::instrument_router(app);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "listening");

    join_set.spawn(async move {
        let result = axum::serve(listener, app).with_graceful_shutdown(shutdown).await;
        ("http", result.context("http server"))
    });

    wait_for_sources(join_set).await
}

async fn wait_for_sources(join_set: JoinSet<SourceResult>) -> anyhow::Result<()> {
    wait_for_sources_with_shutdown(join_set, trogon_telemetry::shutdown_otel).await
}

fn log_nats_connection(server_max_payload: usize, max_payload: MaxPayload) {
    info!(
        server_max_payload_bytes = server_max_payload,
        claim_check_threshold_bytes = max_payload.threshold(),
        "NATS connected"
    );
}

async fn register_telegram_webhooks<C, E, Register>(
    integrations: &[config::SourceIntegration<source::telegram::TelegramSourceConfig>],
    client: impl FnOnce() -> Result<C, E>,
    register: Register,
) where
    E: std::error::Error,
    Register: for<'a> std::ops::AsyncFn(
            &'a source::telegram::TelegramSourceConfig,
            &'a C,
        ) -> Result<(), source::telegram::registration::RegistrationError>,
{
    let registrations: Vec<_> = integrations
        .iter()
        .filter(|integration| integration.config.registration.is_some())
        .collect();
    if !registrations.is_empty() {
        match client() {
            Ok(client) => {
                for integration in registrations {
                    if let Err(error) = register(&integration.config, &client).await {
                        error!(
                            source = "telegram",
                            integration = %integration.id,
                            error = %error,
                            "Telegram webhook registration failed"
                        );
                    }
                }
            }
            Err(error) => {
                error!(
                    source = "telegram",
                    error = %error,
                    "Telegram webhook registration HTTP client initialization failed"
                );
            }
        }
    }
}

fn spawn_discord_gateway<P, S, Run, Running>(
    tasks: &mut JoinSet<SourceResult>,
    config: Option<&source::discord::config::DiscordConfig>,
    publisher: ClaimCheckPublisher<P, S>,
    run: Run,
) where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    Run: FnOnce(ClaimCheckPublisher<P, S>, source::discord::config::DiscordConfig, twilight_gateway::Shard) -> Running
        + Send
        + 'static,
    Running: std::future::Future<Output = ()> + Send + 'static,
{
    if let Some(config) = config {
        let config = config.clone();
        tasks.spawn(async move {
            let shard = twilight_gateway::Shard::new(
                twilight_gateway::ShardId::ONE,
                config.bot_token.as_str().to_owned(),
                config.intents,
            );
            run(publisher, config, shard).await;
            ("discord-gateway", Ok(()))
        });
        info!(source = "discord", "gateway runner spawned");
    }
}

fn spawn_slack_socket_modes<P, S, R, Shutdown, Run, Running>(
    tasks: &mut JoinSet<SourceResult>,
    integrations: &[config::SourceIntegration<source::slack::SlackConfig>],
    publisher: ClaimCheckPublisher<P, S>,
    requester: R,
    shutdown: Shutdown,
    run: Run,
) where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    R: RequestClient,
    Shutdown: std::future::Future<Output = ()> + Clone + Send + 'static,
    Run: FnOnce(
            ClaimCheckPublisher<P, S>,
            R,
            source::slack::SlackConfig,
            source::slack::socket_mode::HttpSocketConnector,
        ) -> Running
        + Clone
        + Send
        + 'static,
    Running: std::future::Future<Output = Result<(), source::slack::socket_mode::SocketModeError>> + Send + 'static,
{
    for (integration, open_url, intended_use) in integrations.iter().filter_map(|integration| {
        let socket_mode = integration.config.socket_mode()?;
        Some((
            integration,
            socket_mode.api_base_url.connections_open_url().to_string(),
            socket_mode.intended_use,
        ))
    }) {
        let publisher = publisher.clone();
        let requester = requester.clone();
        let config = integration.config.clone();
        let shutdown = shutdown.clone();
        let run = run.clone();
        tasks.spawn(async move {
            let result = tokio::select! {
                () = shutdown => Ok(()),
                result = supervision::supervise("slack-socket-mode", SLACK_RESTART_BACKOFF, move || {
                    let connector = source::slack::socket_mode::HttpSocketConnector::new(open_url.clone());
                    run.clone()(publisher.clone(), requester.clone(), config.clone(), connector)
                }) => result.context("slack socket mode"),
            };
            ("slack-socket-mode", result)
        });
        info!(
            source = "slack",
            integration = %integration.id,
            intended_use = %intended_use,
            "socket mode runner spawned"
        );
    }
}

struct PreparedGooglePubSub<'a> {
    integration: &'a config::SourceIntegration<source::google_pubsub::GooglePubSubConfig>,
    source: source::google_pubsub::GooglePubSubSource,
}

fn prepare_google_pubsub_sources(
    integrations: &[config::SourceIntegration<source::google_pubsub::GooglePubSubConfig>],
) -> anyhow::Result<Vec<PreparedGooglePubSub<'_>>> {
    integrations
        .iter()
        .map(|integration| {
            let source::google_pubsub::config::GooglePubSubTransportConfig::StreamingPull(streaming_pull) =
                &integration.config.transport;
            let credentials = streaming_pull
                .credentials
                .build()
                .with_context(|| format!("google_pubsub/{}: invalid credentials", integration.id))?;
            Ok(PreparedGooglePubSub {
                integration,
                source: source::google_pubsub::GooglePubSubSource::new(credentials),
            })
        })
        .collect()
}

fn spawn_google_pubsub_streaming_pulls<P, S, Shutdown, Run, Running>(
    tasks: &mut JoinSet<SourceResult>,
    prepared: Vec<PreparedGooglePubSub<'_>>,
    publisher: ClaimCheckPublisher<P, S>,
    shutdown: Shutdown,
    run: Run,
) where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    Shutdown: std::future::Future<Output = ()> + Clone + Send + 'static,
    Run: FnOnce(
            ClaimCheckPublisher<P, S>,
            source::google_pubsub::GooglePubSubConfig,
            source::google_pubsub::GooglePubSubSource,
            Shutdown,
        ) -> Running
        + Clone
        + Send
        + 'static,
    Running: std::future::Future<Output = ()> + Send + 'static,
{
    for PreparedGooglePubSub { integration, source } in prepared {
        let publisher = publisher.clone();
        let config = integration.config.clone();
        let shutdown = shutdown.clone();
        let run = run.clone();
        tasks.spawn(async move {
            run(publisher, config, source, shutdown).await;
            ("google-pubsub-streaming-pull", Ok(()))
        });
        info!(source = "google_pubsub", integration = %integration.id, "streaming pull runner spawned");
    }
}

async fn wait_for_sources_with_shutdown<E: std::error::Error>(
    mut join_set: JoinSet<SourceResult>,
    shutdown_telemetry: impl FnOnce() -> Result<(), E>,
) -> anyhow::Result<()> {
    let task_count = join_set.len();
    info!(count = task_count, "tasks spawned");

    let mut failed: usize = 0;
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok((name, Ok(()))) => info!(source = name, "task stopped"),
            Ok((name, Err(e))) => {
                error!(source = name, error = %e, "task failed");
                failed += 1;
            }
            Err(e) => {
                error!(error = %e, "task panicked");
                failed += 1;
            }
        }
    }

    info!("all tasks stopped, shutting down");
    if let Err(e) = shutdown_telemetry() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    if failed == task_count {
        anyhow::bail!("all {task_count} task(s) failed");
    }

    Ok(())
}

async fn refresh_credential_runtime_projection(
    event_store: &credential::store::CredentialEventStore,
) -> anyhow::Result<(
    credential_runtime_projection::RuntimeCredentialRegistry,
    credential_runtime_projection::RuntimeProjectionRefreshReport,
)> {
    refresh_credential_runtime_projection_from_start(
        event_store.events_stream(),
        event_store,
        credential_runtime_projection::RuntimeCredentialRegistry::default(),
    )
    .await
}

async fn refresh_credential_runtime_projection_from_start<EventStream, EventStore>(
    event_stream: &EventStream,
    event_store: &EventStore,
    registry: credential_runtime_projection::RuntimeCredentialRegistry,
) -> anyhow::Result<(
    credential_runtime_projection::RuntimeCredentialRegistry,
    credential_runtime_projection::RuntimeProjectionRefreshReport,
)>
where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
{
    let report = registry
        .refresh_from_credential_stream_tracked(event_stream, event_store)
        .await
        .context("credential runtime projection refresh")?;
    log_projection_refresh(&report);

    Ok((registry, report))
}

fn log_projection_refresh(report: &credential_runtime_projection::RuntimeProjectionRefreshReport) {
    let scanned_events = report.scanned_events();
    let decoded_events = report.decoded_events();
    let skipped_events = report.skipped_events();
    let changed_credentials = report.changed_credentials();
    let applied_credentials = report.applied_credentials();
    let projected_integrations = report.projected_integrations();
    let checkpoint_loaded_sequence = report.checkpoint_loaded_sequence();
    let checkpoint_advanced_to = report.checkpoint_advanced_to();
    info!(
        scanned_events,
        decoded_events,
        skipped_events,
        changed_credentials,
        applied_credentials,
        projected_integrations,
        checkpoint_loaded_sequence,
        checkpoint_advanced_to = ?checkpoint_advanced_to,
        "credential runtime projection refreshed"
    );
}

fn credential_management_admin_token_from_env(env: &impl ReadEnv) -> anyhow::Result<Option<SecretString>> {
    match env.var(CREDENTIAL_MANAGEMENT_ADMIN_TOKEN) {
        Ok(value) => SecretString::new(value)
            .map(Some)
            .with_context(|| format!("{CREDENTIAL_MANAGEMENT_ADMIN_TOKEN} must not be empty")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(source) => Err(source).with_context(|| format!("failed to read {CREDENTIAL_MANAGEMENT_ADMIN_TOKEN}")),
    }
}

fn openbao_secret_store_from_env(
    env: &impl ReadEnv,
    enabled_by: &str,
) -> anyhow::Result<secret_store::openbao_secret_store::OpenBaoSecretStore> {
    let address = env
        .var(OPENBAO_ADDR)
        .with_context(|| format!("{OPENBAO_ADDR} must be set when {enabled_by} is enabled"))?;
    let token = env
        .var(OPENBAO_TOKEN)
        .with_context(|| format!("{OPENBAO_TOKEN} must be set when {enabled_by} is enabled"))?;
    let transport = openbao_transport_from_env(env)?;
    secret_store::openbao_secret_store::OpenBaoSecretStore::new(address, token, transport)
        .context("OpenBao secret store configuration")
}

fn openbao_transport_from_env(env: &impl ReadEnv) -> anyhow::Result<OpenBaoTransport> {
    match env.var(OPENBAO_ALLOW_PLAINTEXT) {
        Err(std::env::VarError::NotPresent) => Ok(OpenBaoTransport::TlsRequired),
        Ok(value) if value == "false" => Ok(OpenBaoTransport::TlsRequired),
        Ok(value) if value == "true" => Ok(OpenBaoTransport::PlaintextAllowed),
        Ok(value) => anyhow::bail!("{OPENBAO_ALLOW_PLAINTEXT} must be true or false, got '{value}'"),
        Err(source) => Err(source).with_context(|| format!("failed to read {OPENBAO_ALLOW_PLAINTEXT}")),
    }
}

#[derive(Debug, thiserror::Error)]
enum NotionVerificationTokenCommandError {
    #[error("notion integration '{0}' is not configured")]
    IntegrationNotConfigured(source_integration_id::SourceIntegrationId),
}

async fn notion_verification_token<N, J, W>(
    resolved: &config::ResolvedConfig,
    integration: &source_integration_id::SourceIntegrationId,
    watch: bool,
    nats: &N,
    js_context: &J,
    out: &mut W,
) -> anyhow::Result<()>
where
    N: trogon_nats::SubscribeClient<SubscribeError = async_nats::client::SubscribeError>,
    J: trogon_nats::jetstream::JetStreamGetStream<Error = async_nats::jetstream::context::GetStreamError>,
    J::Stream: trogon_nats::jetstream::JetStreamLastRawMessageBySubject,
    W: Write,
{
    let notion = resolved
        .notion
        .iter()
        .find(|source| &source.id == integration)
        .ok_or_else(|| NotionVerificationTokenCommandError::IntegrationNotConfigured(integration.clone()))?;

    let token = if watch {
        crate::source::notion::verification_token::watch(nats, &notion.config).await?
    } else {
        crate::source::notion::verification_token::latest(js_context, &notion.config).await?
    };

    writeln!(out, "{}", token.as_str())?;
    Ok(())
}

#[cfg(test)]
mod command_tests;
#[cfg(test)]
mod lifecycle_tests;
