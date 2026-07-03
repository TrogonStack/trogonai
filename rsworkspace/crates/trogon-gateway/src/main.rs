#![cfg_attr(coverage, allow(dead_code, unused_imports))] // coverage cfg-excludes main(), orphaning helpers + imports
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[cfg(not(coverage))]
mod cli;
#[cfg_attr(coverage, allow(dead_code))]
mod config;
#[cfg_attr(coverage, allow(dead_code))]
mod constants;
#[cfg_attr(coverage, allow(dead_code))]
mod http;
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

use std::io::Write;
#[cfg(not(coverage))]
use std::net::SocketAddr;
#[cfg(not(coverage))]
use std::time::Duration;

#[cfg(not(coverage))]
use crate::constants::{CLAIM_CHECK_BUCKET, NATS_CONNECT_TIMEOUT, NATS_SERVER_INFO_POLL_INTERVAL};
#[cfg(not(coverage))]
use anyhow::Context;
#[cfg(not(coverage))]
use tokio::task::JoinSet;
#[cfg(not(coverage))]
use tracing::{error, info};
#[cfg(not(coverage))]
use trogon_nats::jetstream::{ClaimCheckPublisher, MaxPayload, NatsJetStreamClient, NatsObjectStore};
#[cfg(not(coverage))]
use trogon_nats::{connect, wait_for_server_info};
#[cfg(not(coverage))]
use trogon_std::args::{CliArgs, ParseArgs};
#[cfg(not(coverage))]
use trogon_std::env::SystemEnv;
#[cfg(not(coverage))]
use trogon_std::fs::SystemFs;

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
        anyhow::bail!("no sources configured — provide a config file with at least one source");
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
    let object_store = NatsObjectStore::provision(
        &js_context,
        async_nats::jetstream::object_store::Config {
            bucket: CLAIM_CHECK_BUCKET.to_string(),
            max_age: Duration::from_secs(8 * 24 * 60 * 60),
            ..Default::default()
        },
    )
    .await?;
    let client = NatsJetStreamClient::new(js_context);

    streams::provision(&client, &resolved).await?;
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

    let publisher = ClaimCheckPublisher::new(
        client.clone(),
        object_store.clone(),
        CLAIM_CHECK_BUCKET.to_string(),
        nats.clone(),
    );

    {
        if let Some(ref cfg) = resolved.discord {
            let p = publisher.clone();
            let discord_cfg = cfg.clone();
            join_set.spawn(async move {
                source::discord::gateway_runner::run(p, &discord_cfg).await;
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

    let app = trogon_std::telemetry::http::instrument_router(http::mount_sources(resolved, publisher));

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
