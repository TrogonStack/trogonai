#![cfg_attr(coverage, allow(dead_code, unused_imports))] // coverage build cfg-excludes the entrypoint, orphaning its private helpers
#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod acp_connection_id;
mod config;
mod connection;
mod constants;
mod transport;

use tokio::sync::{mpsc, watch};
use tracing::info;
use transport::{AppState, ManagerRequest};

#[cfg(not(coverage))]
use {
    acp_nats::nats,
    clap::Parser,
    std::net::SocketAddr,
    tracing::error,
    trogon_std::{env::SystemEnv, fs::SystemFs, signal::shutdown_signal},
    trogon_telemetry::{ResourceAttribute, ServiceName},
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = config::Args::parse();
    let server_config = config::config_from_args(args, &SystemEnv)?;
    trogon_telemetry::init_logger(
        ServiceName::AcpNatsServer,
        [ResourceAttribute::acp_prefix(server_config.acp.acp_prefix())],
        &SystemEnv,
        &SystemFs,
    );
    let server_config = config::apply_timeout_overrides(server_config, &SystemEnv);

    info!("ACP remote transport bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(server_config.acp.nats(), nats_connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(js_context);

    let (shutdown_tx, _) = watch::channel(false);
    let (manager_tx, manager_rx) = mpsc::unbounded_channel::<ManagerRequest>();
    let connection_runtime = build_connection_runtime()?;

    let conn_thread = std::thread::Builder::new().name(THREAD_NAME.into()).spawn(move || {
        run_connection_thread(
            connection_runtime,
            manager_rx,
            nats_client,
            js_client,
            server_config.acp,
        )
    })?;

    let state = AppState {
        bind_host: server_config.host,
        manager_tx,
        shutdown_tx: shutdown_tx.clone(),
    };

    let app = trogon_std::telemetry::http::instrument_router(
        axum::Router::new()
            .route(
                ACP_ENDPOINT,
                axum::routing::get(transport::get)
                    .post(transport::post)
                    .delete(transport::delete),
            )
            .with_state(state),
    );

    let addr = SocketAddr::from((server_config.host, server_config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(address = %addr, "Listening for ACP transport connections");

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            info!("Shutdown signal received, stopping server");
            let _ = shutdown_tx.send(true);
        })
        .await;

    match &result {
        Ok(()) => info!("ACP remote transport bridge stopped"),
        Err(e) => error!(error = %e, "ACP remote transport bridge stopped with error"),
    }

    // `serve` returning drops the Router (and its AppState.conn_tx), which
    // closes the channel and lets the connection thread's recv-loop exit and
    // drain active connections. Wait for that drain to finish before tearing
    // down telemetry.
    if let Err(e) = conn_thread.join() {
        error!("Connection thread panicked: {e:?}");
    }

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result.map_err(Into::into)
}

#[cfg(coverage)]
fn main() {}

use constants::{ACP_ENDPOINT, THREAD_NAME};

fn build_connection_runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| anyhow::anyhow!("failed to create per-connection runtime: {error}"))
}

/// Runs a single-threaded tokio runtime with a
/// `LocalSet`. All WebSocket connections are processed here because the ACP
/// `Agent` trait is `?Send`, requiring `spawn_local` / `Rc`.
fn run_connection_thread<N, J>(
    rt: tokio::runtime::Runtime,
    manager_rx: mpsc::UnboundedReceiver<ManagerRequest>,
    nats_client: N,
    js_client: J,
    config: acp_nats::Config,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + Send + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(process_connections(manager_rx, nats_client, js_client, config)));

    // run_until returns once process_connections completes, but
    // sub-tasks spawned by connection handlers (pumps,
    // AgentSideConnection internals) may still be live on the
    // LocalSet. Drive them for a bounded window so WebSocket close
    // frames are sent and per-connection cleanup finishes, without
    // blocking forever on stuck sub-tasks (e.g. a hanging NATS
    // request that never resolves).
    let drain = std::time::Duration::from_secs(5);
    rt.block_on(local.run_until(async { tokio::time::sleep(drain).await }));
    info!("Local thread exiting");
}

async fn process_connections<N, J>(
    mut manager_rx: mpsc::UnboundedReceiver<ManagerRequest>,
    nats_client: N,
    js_client: J,
    config: acp_nats::Config,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    let mut websocket_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut http_connection_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut http_connections = std::collections::HashMap::new();

    while let Some(request) = manager_rx.recv().await {
        transport::process_manager_request(
            request,
            &mut http_connections,
            &mut websocket_handles,
            &mut http_connection_handles,
            &nats_client,
            &js_client,
            &config,
        )
        .await;
    }

    let active = websocket_handles.iter().filter(|h| !h.is_finished()).count()
        + http_connection_handles.iter().filter(|h| !h.is_finished()).count();
    info!(
        active_connections = active,
        "Connection channel closed, draining active connections"
    );

    for handle in websocket_handles {
        let _ = handle.await;
    }
    for handle in http_connection_handles {
        let _ = handle.await;
    }

    info!("All connections drained");
}

#[cfg(test)]
mod tests;
