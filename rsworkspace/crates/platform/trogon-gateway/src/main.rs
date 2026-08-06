#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[cfg(not(coverage))]
mod cli;
#[cfg_attr(coverage, allow(dead_code))]
mod config;
#[cfg_attr(coverage, allow(dead_code))]
mod constants;
#[cfg_attr(coverage, allow(dead_code))]
mod credential;
#[cfg_attr(coverage, allow(dead_code))]
mod credential_management;
#[cfg_attr(coverage, allow(dead_code))]
mod credential_management_idempotency;
#[cfg_attr(coverage, allow(dead_code))]
mod http;
#[cfg_attr(coverage, allow(dead_code))]
mod secret_store;
#[cfg_attr(coverage, allow(dead_code))]
mod source;
#[cfg_attr(coverage, allow(dead_code))]
mod source_integration_id;
#[cfg_attr(coverage, allow(dead_code))]
mod source_plugin;
#[cfg_attr(coverage, allow(dead_code))]
mod source_status;
#[cfg_attr(coverage, allow(dead_code))]
mod streams;

use std::error::Error;
use std::io::Write;
#[cfg(not(coverage))]
use std::net::SocketAddr;
#[cfg(not(coverage))]
use std::time::Duration;

#[cfg(not(coverage))]
use anyhow::Context;
#[cfg(not(coverage))]
use async_nats::jetstream::kv;
#[cfg(not(coverage))]
use tokio::task::JoinSet;
#[cfg(not(coverage))]
use tracing::{error, info};
#[cfg(not(coverage))]
use trogon_decider_runtime::StreamRead;
#[cfg(not(coverage))]
use trogon_nats::jetstream::{
    ClaimBucket, ClaimCheckPublisher, ClaimRetention, JetStreamGetRawMessage, JetStreamGetStreamInfo, MaxPayload,
    NatsJetStreamClient, NatsObjectStore,
};
#[cfg(not(coverage))]
use trogon_nats::{connect, wait_for_server_info};
#[cfg(not(coverage))]
use trogon_std::SecretString;
#[cfg(not(coverage))]
use trogon_std::args::{CliArgs, ParseArgs};
#[cfg(not(coverage))]
use trogon_std::env::SystemEnv;
#[cfg(not(coverage))]
use trogon_std::fs::SystemFs;

#[cfg(not(coverage))]
use crate::constants::{CLAIM_CHECK_TTL_GRACE, NATS_CONNECT_TIMEOUT, NATS_SERVER_INFO_POLL_INTERVAL};
#[cfg(not(coverage))]
const ENABLE_RUNTIME_DISCORD_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_DISCORD_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_GITHUB_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_GITHUB_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_GITLAB_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_GITLAB_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_LINEAR_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_LINEAR_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_NOTION_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_NOTION_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_SENTRY_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_SENTRY_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_SLACK_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_SLACK_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_TELEGRAM_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_TELEGRAM_CREDENTIALS";
#[cfg(not(coverage))]
const ENABLE_RUNTIME_TWITTER_CREDENTIALS: &str = "TROGON_GATEWAY_ENABLE_RUNTIME_TWITTER_CREDENTIALS";
#[cfg(not(coverage))]
const CREDENTIAL_MANAGEMENT_ADMIN_TOKEN: &str = "TROGON_GATEWAY_CREDENTIAL_MANAGEMENT_ADMIN_TOKEN";
#[cfg(not(coverage))]
const OPENBAO_ADDR: &str = "OPENBAO_ADDR";
#[cfg(not(coverage))]
const OPENBAO_TOKEN: &str = "OPENBAO_TOKEN";
#[cfg(not(coverage))]
const CREDENTIAL_WORKER_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(not(coverage))]
const CREDENTIAL_RUNTIME_PROJECTION_WORKER_INTERVAL: Duration = Duration::from_secs(30);

#[cfg(not(coverage))]
type SourceResult = (&'static str, anyhow::Result<()>);

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = CliArgs::<cli::Cli>::new().parse_args();
    let resolved = config::load_with_overrides(cli.runtime.config.as_deref(), &cli.runtime.nats)?;

    match cli.command {
        cli::Command::Serve => serve(resolved).await,
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

#[cfg(not(coverage))]
async fn serve(resolved: config::ResolvedConfig) -> anyhow::Result<()> {
    if !resolved.has_any_source() {
        anyhow::bail!("no sources configured - provide a config file with at least one source");
    }

    trogon_telemetry::init_logger(trogon_telemetry::ServiceName::TrogonGateway, [], &SystemEnv, &SystemFs);

    info!("trogon-gateway starting");

    let nats = connect(&resolved.nats, NATS_CONNECT_TIMEOUT).await?;
    let server_max_payload = wait_for_server_info(&nats, NATS_CONNECT_TIMEOUT, NATS_SERVER_INFO_POLL_INTERVAL)
        .await?
        .max_payload;
    let max_payload = MaxPayload::from_server_limit(server_max_payload);
    info!(
        server_max_payload_bytes = server_max_payload,
        claim_check_threshold_bytes = max_payload.threshold(),
        "NATS connected"
    );
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
        stream = credential::stream::CREDENTIAL_STREAM,
        snapshot_bucket = credential::store::CREDENTIAL_SNAPSHOT_BUCKET,
        "credential event store opened"
    );
    let (runtime_credentials, _credential_projection_refresh_report, runtime_projection_checkpoints) =
        refresh_credential_runtime_projection(&credential_store).await?;
    let telegram_registration_configs: Vec<_> = resolved
        .telegram
        .iter()
        .filter(|integration| integration.config.registration.is_some())
        .map(|integration| (&integration.id, &integration.config))
        .collect();
    if !telegram_registration_configs.is_empty() {
        match crate::source::telegram::registration::registration_http_client() {
            Ok(telegram_http_client) => {
                for (integration_id, config) in telegram_registration_configs {
                    if let Err(error) =
                        crate::source::telegram::registration::register_webhook(config, &telegram_http_client).await
                    {
                        error!(
                            source = "telegram",
                            integration = %integration_id,
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

    let port = resolved.http_server.port;
    let mut join_set: JoinSet<SourceResult> = JoinSet::new();

    let publisher = ClaimCheckPublisher::new(client.clone(), claim_binding, nats.clone());
    {
        let projection_runtime_credentials = runtime_credentials.clone();
        let projection_event_stream = credential_store.events_stream().clone();
        let projection_event_store = credential_store.clone();
        join_set.spawn(async move {
            credential::processor::runtime_projection::run_checkpointed_refresh_worker(
                projection_runtime_credentials,
                projection_event_stream,
                projection_event_store,
                runtime_projection_checkpoints,
                CREDENTIAL_RUNTIME_PROJECTION_WORKER_INTERVAL,
            )
            .await;
            ("credential-runtime-projection-worker", Ok(()))
        });
        info!("credential runtime projection refresh worker spawned");
    }
    let discord_runtime_credentials = discord_runtime_credentials_from_env(&runtime_credentials)?;

    {
        if let Some(ref cfg) = resolved.discord {
            let p = publisher.clone();
            let discord_cfg = cfg.clone();
            let runtime_credentials = discord_runtime_credentials.clone();
            join_set.spawn(async move {
                if let Some(runtime_credentials) = runtime_credentials {
                    crate::source::discord::gateway_runner::run_with_runtime_credentials(
                        p,
                        &discord_cfg,
                        runtime_credentials,
                    )
                    .await;
                } else {
                    crate::source::discord::gateway_runner::run(p, &discord_cfg).await;
                }
                ("discord-gateway", Ok(()))
            });
            info!(source = "discord", "gateway runner spawned");
        }
    }

    for integration in resolved
        .slack
        .iter()
        .filter(|integration| integration.config.socket_mode().is_some())
    {
        let p = publisher.clone();
        let integration_id = integration.id.clone();
        let slack_cfg = integration.config.clone();
        join_set.spawn(async move {
            let result = tokio::select! {
                _ = trogon_std::signal::shutdown_signal() => Ok(()),
                result = crate::source::slack::socket_mode::run(p, &slack_cfg) => {
                    result.context("slack socket mode")
                }
            };
            ("slack-socket-mode", result)
        });
        info!(
            source = "slack",
            integration = %integration_id,
            "socket mode runner spawned"
        );
    }

    let app = match source_runtime_credentials_from_env(&runtime_credentials)? {
        Some(runtime_credentials) => {
            http::mount_sources_with_runtime_credentials(resolved, publisher, runtime_credentials)
        }
        None => http::mount_sources(resolved, publisher),
    };
    let app = match credential_management_admin_token_from_env()? {
        Some(admin_token) => {
            let store = openbao_secret_store_from_env(CREDENTIAL_MANAGEMENT_ADMIN_TOKEN)?;
            let idempotency = credential_management_idempotency::open(client.context().clone()).await?;
            let worker_checkpoints =
                credential::processor::recovery_worker::open_checkpoint_store(client.context().clone()).await?;
            let recovery_status_checkpoints = worker_checkpoints.clone();
            let worker_event_stream = credential_store.events_stream().clone();
            let worker_event_store = credential_store.clone();
            let worker_handler_event_store = credential_store.clone();
            let worker_store = store.clone();
            let worker_runtime_credentials = runtime_credentials.clone();
            join_set.spawn(async move {
                let handler = credential::handler::CredentialRuntimeHandler::new(
                    worker_handler_event_store,
                    worker_store,
                    worker_runtime_credentials,
                );
                credential::processor::recovery_worker::run(
                    worker_event_stream,
                    worker_event_store,
                    handler,
                    worker_checkpoints,
                    CREDENTIAL_WORKER_INTERVAL,
                )
                .await;
                ("credential-worker", Ok(()))
            });
            info!("credential recovery worker spawned");
            info!("credential management API enabled");
            let credential_management_routes = credential_management::router(
                credential_store.clone(),
                store,
                runtime_credentials.clone(),
                admin_token.clone(),
                idempotency,
            )
            .merge(credential_management::recovery_status_router(
                admin_token,
                recovery_status_checkpoints,
            ));
            app.nest("/-/credentials", credential_management_routes)
        }
        None => app,
    };
    let app = trogon_std::telemetry::http::instrument_router(app);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "listening");

    join_set.spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(trogon_std::signal::shutdown_signal())
            .await;
        ("http", result.context("http server"))
    });

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
    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    if failed == task_count {
        anyhow::bail!("all {task_count} task(s) failed");
    }

    Ok(())
}

#[cfg(not(coverage))]
async fn refresh_credential_runtime_projection(
    event_store: &credential::store::CredentialEventStore,
) -> anyhow::Result<(
    credential::processor::runtime_projection::RuntimeCredentialRegistry,
    credential::processor::runtime_projection::RuntimeProjectionRefreshReport,
    credential::processor::runtime_projection::RuntimeProjectionKvCheckpointStore<kv::Store>,
)> {
    let registry = credential::processor::runtime_projection::RuntimeCredentialRegistry::default();
    let checkpoints = credential::processor::runtime_projection::open_runtime_projection_checkpoint_store(
        event_store.as_jetstream().clone(),
    )
    .await?;
    let (registry, report) = refresh_credential_runtime_projection_with_checkpoints(
        event_store.events_stream(),
        event_store,
        &checkpoints,
        registry,
    )
    .await?;
    Ok((registry, report, checkpoints))
}

#[cfg(not(coverage))]
async fn refresh_credential_runtime_projection_with_checkpoints<EventStream, EventStore, Checkpoints>(
    event_stream: &EventStream,
    event_store: &EventStore,
    checkpoints: &Checkpoints,
    registry: credential::processor::runtime_projection::RuntimeCredentialRegistry,
) -> anyhow::Result<(
    credential::processor::runtime_projection::RuntimeCredentialRegistry,
    credential::processor::runtime_projection::RuntimeProjectionRefreshReport,
)>
where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    Checkpoints: credential::processor::runtime_projection::RuntimeProjectionCheckpointStore,
{
    let report = registry
        .refresh_from_credential_stream_checkpointed(event_stream, event_store, checkpoints)
        .await
        .context("credential runtime projection refresh")?;
    info!(
        scanned_events = report.scanned_events(),
        decoded_events = report.decoded_events(),
        skipped_events = report.skipped_events(),
        changed_credentials = report.changed_credentials(),
        applied_credentials = report.applied_credentials(),
        projected_integrations = report.projected_integrations(),
        checkpoint_loaded_sequence = report.checkpoint_loaded_sequence(),
        checkpoint_advanced_to = ?report.checkpoint_advanced_to(),
        "credential runtime projection refreshed"
    );

    Ok((registry, report))
}

#[cfg(not(coverage))]
fn source_runtime_credentials_from_env(
    runtime_credentials: &credential::processor::runtime_projection::RuntimeCredentialRegistry,
) -> anyhow::Result<
    Option<source_plugin::RuntimeCredentialMounts<secret_store::openbao_secret_store::OpenBaoSecretStore>>,
> {
    let github_enabled = env_flag_enabled(ENABLE_RUNTIME_GITHUB_CREDENTIALS);
    let gitlab_enabled = env_flag_enabled(ENABLE_RUNTIME_GITLAB_CREDENTIALS);
    let incidentio_enabled = env_flag_enabled(ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS);
    let linear_enabled = env_flag_enabled(ENABLE_RUNTIME_LINEAR_CREDENTIALS);
    let microsoft_graph_enabled = env_flag_enabled(ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS);
    let notion_enabled = env_flag_enabled(ENABLE_RUNTIME_NOTION_CREDENTIALS);
    let sentry_enabled = env_flag_enabled(ENABLE_RUNTIME_SENTRY_CREDENTIALS);
    let slack_enabled = env_flag_enabled(ENABLE_RUNTIME_SLACK_CREDENTIALS);
    let telegram_enabled = env_flag_enabled(ENABLE_RUNTIME_TELEGRAM_CREDENTIALS);
    let twitter_enabled = env_flag_enabled(ENABLE_RUNTIME_TWITTER_CREDENTIALS);
    if !github_enabled
        && !gitlab_enabled
        && !incidentio_enabled
        && !linear_enabled
        && !microsoft_graph_enabled
        && !notion_enabled
        && !sentry_enabled
        && !slack_enabled
        && !telegram_enabled
        && !twitter_enabled
    {
        return Ok(None);
    }

    let enabled_by = match (
        github_enabled,
        gitlab_enabled,
        incidentio_enabled,
        linear_enabled,
        microsoft_graph_enabled,
        notion_enabled,
        sentry_enabled,
        slack_enabled,
        telegram_enabled,
        twitter_enabled,
    ) {
        (true, false, false, false, false, false, false, false, false, false) => ENABLE_RUNTIME_GITHUB_CREDENTIALS,
        (false, true, false, false, false, false, false, false, false, false) => ENABLE_RUNTIME_GITLAB_CREDENTIALS,
        (false, false, true, false, false, false, false, false, false, false) => ENABLE_RUNTIME_INCIDENTIO_CREDENTIALS,
        (false, false, false, true, false, false, false, false, false, false) => ENABLE_RUNTIME_LINEAR_CREDENTIALS,
        (false, false, false, false, true, false, false, false, false, false) => {
            ENABLE_RUNTIME_MICROSOFT_GRAPH_CREDENTIALS
        }
        (false, false, false, false, false, true, false, false, false, false) => ENABLE_RUNTIME_NOTION_CREDENTIALS,
        (false, false, false, false, false, false, true, false, false, false) => ENABLE_RUNTIME_SENTRY_CREDENTIALS,
        (false, false, false, false, false, false, false, true, false, false) => ENABLE_RUNTIME_SLACK_CREDENTIALS,
        (false, false, false, false, false, false, false, false, true, false) => ENABLE_RUNTIME_TELEGRAM_CREDENTIALS,
        (false, false, false, false, false, false, false, false, false, true) => ENABLE_RUNTIME_TWITTER_CREDENTIALS,
        _ => "runtime source credential flags",
    };
    let store = openbao_secret_store_from_env(enabled_by)?;
    let resolver = runtime_credentials.resolver(store);

    if github_enabled {
        info!("GitHub webhook runtime credentials enabled");
    }
    if gitlab_enabled {
        info!("GitLab webhook runtime credentials enabled");
    }
    if incidentio_enabled {
        info!("Incident.io webhook runtime credentials enabled");
    }
    if linear_enabled {
        info!("Linear webhook runtime credentials enabled");
    }
    if microsoft_graph_enabled {
        info!("Microsoft Graph webhook runtime credentials enabled");
    }
    if notion_enabled {
        info!("Notion webhook runtime credentials enabled");
    }
    if sentry_enabled {
        info!("Sentry webhook runtime credentials enabled");
    }
    if slack_enabled {
        info!("Slack webhook runtime credentials enabled");
    }
    if telegram_enabled {
        info!("Telegram webhook runtime credentials enabled");
    }
    if twitter_enabled {
        info!("Twitter/X webhook runtime credentials enabled");
    }

    Ok(Some(source_plugin::RuntimeCredentialMounts {
        github: github_enabled.then(|| resolver.clone()),
        gitlab: gitlab_enabled.then(|| resolver.clone()),
        incidentio: incidentio_enabled.then(|| resolver.clone()),
        linear: linear_enabled.then(|| resolver.clone()),
        microsoft_graph: microsoft_graph_enabled.then(|| resolver.clone()),
        notion: notion_enabled.then(|| resolver.clone()),
        sentry: sentry_enabled.then(|| resolver.clone()),
        slack: slack_enabled.then(|| resolver.clone()),
        telegram: telegram_enabled.then(|| resolver.clone()),
        twitter: twitter_enabled.then_some(resolver),
    }))
}

#[cfg(not(coverage))]
fn discord_runtime_credentials_from_env(
    runtime_credentials: &credential::processor::runtime_projection::RuntimeCredentialRegistry,
) -> anyhow::Result<
    Option<
        credential::processor::runtime_projection::RuntimeCredentialResolver<
            secret_store::openbao_secret_store::OpenBaoSecretStore,
        >,
    >,
> {
    if !env_flag_enabled(ENABLE_RUNTIME_DISCORD_CREDENTIALS) {
        return Ok(None);
    }

    let store = openbao_secret_store_from_env(ENABLE_RUNTIME_DISCORD_CREDENTIALS)?;
    info!("Discord gateway runtime credentials enabled");
    Ok(Some(runtime_credentials.resolver(store)))
}

#[cfg(not(coverage))]
fn credential_management_admin_token_from_env() -> anyhow::Result<Option<SecretString>> {
    match std::env::var(CREDENTIAL_MANAGEMENT_ADMIN_TOKEN) {
        Ok(value) => SecretString::new(value)
            .map(Some)
            .with_context(|| format!("{CREDENTIAL_MANAGEMENT_ADMIN_TOKEN} must not be empty")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(source) => Err(source).with_context(|| format!("failed to read {CREDENTIAL_MANAGEMENT_ADMIN_TOKEN}")),
    }
}

#[cfg(not(coverage))]
fn openbao_secret_store_from_env(
    enabled_by: &str,
) -> anyhow::Result<secret_store::openbao_secret_store::OpenBaoSecretStore> {
    let address = std::env::var(OPENBAO_ADDR)
        .with_context(|| format!("{OPENBAO_ADDR} must be set when {enabled_by} is enabled"))?;
    let token = std::env::var(OPENBAO_TOKEN)
        .with_context(|| format!("{OPENBAO_TOKEN} must be set when {enabled_by} is enabled"))?;
    secret_store::openbao_secret_store::OpenBaoSecretStore::new(address, token)
        .context("OpenBao secret store configuration")
}

#[cfg(not(coverage))]
fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
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

#[cfg(coverage)]
fn main() {}

#[cfg(test)]
mod command_tests;

#[cfg(all(coverage, test))]
mod tests;
