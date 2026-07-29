#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod compat;
mod component;
mod config;
mod constants;

use component::NatsAgentComponent;
use tokio::sync::watch;

#[cfg(not(coverage))]
use {
    acp_nats::nats,
    clap::Parser,
    std::net::SocketAddr,
    tracing::{error, info},
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
    let app = build_router(
        nats_client,
        js_client,
        server_config.acp,
        server_config.host,
        &shutdown_tx,
    );

    let addr = SocketAddr::from((server_config.host, server_config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(address = %addr, "Listening for ACP transport connections");

    let drain_tx = shutdown_tx.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            info!("Shutdown signal received, stopping server");
            // Every live connection selects on this, so signalling here is what
            // ends them: `axum` only stops accepting, it cannot close an
            // in-flight SSE or WebSocket stream on its own.
            let _ = drain_tx.send(true);
        })
        .await;

    match &result {
        Ok(()) => info!("ACP remote transport bridge stopped"),
        Err(e) => error!(error = %e, "ACP remote transport bridge stopped with error"),
    }

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result.map_err(Into::into)
}

#[cfg(coverage)]
fn main() {}

/// Builds the served router: the SDK's transport, plus the behaviors it omits.
///
/// The factory runs once per connection, so process-wide values (the NATS
/// clients, the config, the instrumentation scope) are created here and cloned
/// in rather than resolved per connection.
fn build_router<N, J>(
    nats_client: N,
    js_client: J,
    acp_config: acp_nats::Config,
    bind_host: std::net::IpAddr,
    shutdown_tx: &watch::Sender<bool>,
) -> axum::Router
where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + Sync
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + Clone + Send + Sync + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    let meter = trogon_telemetry::meter("acp-nats-server");
    let shutdown_for_factory = shutdown_tx.clone();

    let server = agent_client_protocol_http::AcpHttpServer::new(move || {
        NatsAgentComponent::new(
            nats_client.clone(),
            js_client.clone(),
            acp_config.clone(),
            meter.clone(),
            shutdown_for_factory.subscribe(),
        )
    });

    trogon_std::telemetry::http::instrument_router(compat::apply_layers(server.into_router(), bind_host))
}

#[cfg(test)]
mod tests;
