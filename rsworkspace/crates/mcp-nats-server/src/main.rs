#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
#![cfg_attr(coverage, allow(dead_code, unused_imports))]

mod allowed_host;
mod config;
mod constants;
mod runtime;

#[cfg(not(coverage))]
use {
    crate::constants::MCP_ENDPOINT,
    anyhow::Result,
    axum::Router,
    tokio::net::TcpListener,
    tracing::{error, info},
    trogon_std::{env::SystemEnv, fs::SystemFs, signal::shutdown_signal},
    trogon_telemetry::{ResourceAttribute, ServiceName},
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<()> {
    let config = config::base_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;
    let config::HttpBridgeConfig {
        mcp,
        client_id_prefix,
        server_id,
        bind_addr,
        allowed_hosts,
    } = config;
    let mcp = mcp_nats::apply_timeout_overrides(mcp, &SystemEnv);
    trogon_telemetry::init_logger(
        ServiceName::McpNatsServer,
        [ResourceAttribute::mcp_prefix(mcp.prefix_str())],
        &SystemEnv,
        &SystemFs,
    );

    let nats_connect_timeout = mcp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = mcp_nats::nats::connect(mcp.nats(), nats_connect_timeout).await?;
    let client_ids = runtime::ClientIdFactory::new(client_id_prefix);
    let http_config = runtime::streamable_http_config(allowed_hosts);
    let service = runtime::streamable_http_service(nats_client, mcp, client_ids, server_id, http_config);
    let app = Router::new().route_service(MCP_ENDPOINT, service);
    let listener = TcpListener::bind(bind_addr).await?;

    info!(endpoint = %format!("http://{bind_addr}{MCP_ENDPOINT}"), "MCP HTTP bridge starting");

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    match &result {
        Ok(()) => info!("MCP HTTP bridge stopped"),
        Err(error) => error!(error = %error, "MCP HTTP bridge stopped with error"),
    }

    if let Err(error) = trogon_telemetry::shutdown_otel() {
        error!(error = %error, "OpenTelemetry shutdown failed");
    }

    result?;
    Ok(())
}

#[cfg(coverage)]
fn main() {}
