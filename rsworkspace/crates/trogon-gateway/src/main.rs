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
        .map(|integration| &integration.config)
        .collect();
    if !telegram_registration_configs.is_empty() {
        let telegram_http_client = crate::source::telegram::registration::registration_http_client()?;
        for config in telegram_registration_configs {
            crate::source::telegram::registration::register_webhook(config, &telegram_http_client).await?;
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
                crate::source::discord::gateway_runner::run(p, &discord_cfg).await;
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

#[cfg(coverage)]
fn main() {}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
