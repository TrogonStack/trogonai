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
mod source_status;
#[cfg_attr(coverage, allow(dead_code))]
mod streams;

use std::fmt;
use std::io::Write;
#[cfg(not(coverage))]
use std::net::SocketAddr;
#[cfg(not(coverage))]
use std::time::Duration;

#[cfg(not(coverage))]
use tokio::task::JoinSet;
#[cfg(not(coverage))]
use tracing::{error, info};
#[cfg(not(coverage))]
use trogon_nats::connect;
#[cfg(not(coverage))]
use trogon_nats::jetstream::{ClaimCheckPublisher, MaxPayload, NatsJetStreamClient, NatsObjectStore};
#[cfg(not(coverage))]
use trogon_std::args::{CliArgs, ParseArgs};
#[cfg(not(coverage))]
use trogon_gateway::source;
#[cfg(not(coverage))]
use trogon_std::env::SystemEnv;
#[cfg(not(coverage))]
use trogon_std::fs::SystemFs;

#[cfg(not(coverage))]
const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(not(coverage))]
const CLAIM_CHECK_BUCKET: &str = "trogon-claims";

#[cfg(not(coverage))]
type SourceResult = (&'static str, Result<(), String>);

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
async fn serve(resolved: config::ResolvedConfig) -> Result<(), Box<dyn std::error::Error>> {
    if !resolved.has_any_source() {
        return Err("no sources configured — provide a config file with at least one source".into());
    }

    trogon_telemetry::init_logger(trogon_telemetry::ServiceName::TrogonGateway, [], &SystemEnv, &SystemFs);

    info!("trogon-gateway starting");

    let nats = connect(&resolved.nats, NATS_CONNECT_TIMEOUT).await?;
    // TODO: restore the line below once the async-nats race is resolved.
    //
    // `trogon_nats::connect` always sets `retry_on_initial_connect`, which causes
    // async-nats to return the `Client` before the background connection task has
    // populated the `ServerInfo` watch channel. Reading `server_info().max_payload`
    // immediately after `connect()` therefore returns 0 (the `Default` value for
    // `usize`), which collapses `MaxPayload::from_server_limit` to a threshold of 0
    // and routes every payload — including tiny ones — through the object-store
    // claim-check path.
    //
    // Until async-nats guarantees `server_info()` is populated before returning the
    // `Client` (or `trogon_nats::connect` exposes a post-connection hook), the value
    // is taken from config (`TROGON_GATEWAY_NATS_MAX_PAYLOAD_BYTES`, default 1 MiB).
    //
    // Tracking issue: https://github.com/TrogonStack/trogonai/issues/122
    // let server_max_payload = nats.server_info().max_payload;
    let server_max_payload = resolved.nats_max_payload_bytes.get();
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
        max_payload,
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

    let app = trogon_std::telemetry::http::instrument_router(http::mount_sources(resolved, publisher));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "listening");

    join_set.spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(trogon_std::signal::shutdown_signal())
            .await;
        ("http", result.map_err(|e| format!("http server: {e}")))
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
        return Err(format!("all {task_count} task(s) failed").into());
    }

    Ok(())
}

#[derive(Debug)]
enum NotionVerificationTokenCommandError {
    IntegrationNotConfigured(source_integration_id::SourceIntegrationId),
}

impl fmt::Display for NotionVerificationTokenCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntegrationNotConfigured(integration) => {
                write!(f, "notion integration '{integration}' is not configured")
            }
        }
    }
}

impl std::error::Error for NotionVerificationTokenCommandError {}

async fn notion_verification_token<N, J, W>(
    resolved: &config::ResolvedConfig,
    integration: &source_integration_id::SourceIntegrationId,
    watch: bool,
    nats: &N,
    js_context: &J,
    out: &mut W,
) -> Result<(), Box<dyn std::error::Error>>
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
mod command_tests {
    use super::*;
    use async_nats::jetstream::message::StreamMessage;
    use bytes::Bytes;
    use time::OffsetDateTime;
    use trogon_nats::MockNatsClient;
    use trogon_nats::jetstream::MockJetStreamConsumerFactory;

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
}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
