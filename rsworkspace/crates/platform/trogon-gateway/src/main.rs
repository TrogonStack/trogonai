#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod cli;
mod config;
mod constants;
mod http;
mod source;
mod source_integration_id;
mod source_plugin;
mod source_status;
mod streams;

use std::io::Write;
use std::net::SocketAddr;

use crate::constants::{CLAIM_CHECK_TTL_GRACE, NATS_CONNECT_TIMEOUT, NATS_SERVER_INFO_POLL_INTERVAL};
use anyhow::Context;
use futures_util::FutureExt;
use tokio::task::JoinSet;
use tracing::{error, info};
use trogon_nats::jetstream::{
    ClaimBucket, ClaimCheckPublisher, ClaimRetention, JetStreamPublisher, MaxPayload, NatsJetStreamClient,
    NatsObjectStore, ObjectStorePut,
};
use trogon_nats::{connect, wait_for_server_info};
use trogon_std::args::{CliArgs, ParseArgs};
use trogon_std::env::SystemEnv;
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
    if !resolved.has_any_source() {
        anyhow::bail!("no sources configured — provide a config file with at least one source");
    }

    trogon_telemetry::init_logger(trogon_telemetry::ServiceName::TrogonGateway, [], &SystemEnv, &SystemFs);

    info!("trogon-gateway starting");

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
        shutdown.clone(),
        crate::source::slack::socket_mode::run,
    );

    let app = trogon_std::telemetry::http::instrument_router(http::mount_sources(resolved, publisher));

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

fn spawn_slack_socket_modes<P, S, Shutdown, Run, Running>(
    tasks: &mut JoinSet<SourceResult>,
    integrations: &[config::SourceIntegration<source::slack::SlackConfig>],
    publisher: ClaimCheckPublisher<P, S>,
    shutdown: Shutdown,
    run: Run,
) where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    Shutdown: std::future::Future<Output = ()> + Clone + Send + 'static,
    Run: FnOnce(
            ClaimCheckPublisher<P, S>,
            source::slack::SlackConfig,
            source::slack::socket_mode::HttpSocketConnector,
        ) -> Running
        + Clone
        + Send
        + 'static,
    Running: std::future::Future<Output = Result<(), source::slack::socket_mode::SocketModeError>> + Send + 'static,
{
    for integration in integrations
        .iter()
        .filter(|integration| integration.config.socket_mode().is_some())
    {
        let publisher = publisher.clone();
        let config = integration.config.clone();
        let shutdown = shutdown.clone();
        let run = run.clone();
        tasks.spawn(async move {
            let result = tokio::select! {
                () = shutdown => Ok(()),
                result = async {
                    let connector = source::slack::socket_mode::HttpSocketConnector::slack();
                    run(publisher, config, connector).await
                } => result.context("slack socket mode"),
            };
            ("slack-socket-mode", result)
        });
        info!(source = "slack", integration = %integration.id, "socket mode runner spawned");
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
