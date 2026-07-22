#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod config;

use std::future::Future;

use anyhow::Result;
use rmcp::service::{RoleClient, RoleServer};
use rmcp::transport::Transport;
#[cfg(not(coverage))]
use rmcp::transport::async_rw::AsyncRwTransport;
#[cfg(not(coverage))]
use rmcp::transport::stdio;
use tracing::{error, info};

#[cfg(not(coverage))]
use trogon_std::{env::SystemEnv, fs::SystemFs, signal::shutdown_signal};
#[cfg(not(coverage))]
use trogon_telemetry::{ResourceAttribute, ServiceName};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<()> {
    let config = config::base_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;
    let config::BridgeConfig {
        mcp,
        client_id,
        server_id,
    } = config;
    let mcp = mcp_nats::apply_timeout_overrides(mcp, &SystemEnv);
    trogon_telemetry::init_logger(
        ServiceName::McpNatsStdio,
        [ResourceAttribute::mcp_prefix(mcp.prefix_str())],
        &SystemEnv,
        &SystemFs,
    );

    info!(
        mcp_prefix = %mcp.prefix_str(),
        client_id = %client_id,
        server_id = %server_id,
        "MCP bridge starting"
    );

    let nats_connect_timeout = mcp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = mcp_nats::nats::connect(mcp.nats(), nats_connect_timeout).await?;
    let remote = mcp_nats::client::connect(nats_client, &mcp, client_id, server_id).await?;
    let (stdin, stdout) = stdio();
    let local = AsyncRwTransport::<RoleServer, _, _>::new_server(stdin, stdout);

    let result = run_bridge(local, remote, shutdown_signal()).await;

    match &result {
        Ok(()) => info!("MCP bridge stopped"),
        Err(error) => error!(error = %error, "MCP bridge stopped with error"),
    }

    if let Err(error) = trogon_telemetry::shutdown_otel() {
        error!(error = %error, "OpenTelemetry shutdown failed");
    }

    result
}

#[cfg(coverage)]
fn main() {}

async fn run_bridge<L, R, S>(mut local: L, mut remote: R, shutdown_signal: S) -> Result<()>
where
    L: Transport<RoleServer>,
    R: Transport<RoleClient>,
    S: Future<Output = ()> + Send,
{
    info!("MCP bridge running on stdio with NATS client proxy");

    let result = {
        tokio::pin!(shutdown_signal);
        loop {
            tokio::select! {
                local_message = local.receive() => {
                    let Some(local_message) = local_message else {
                        info!("MCP bridge shutting down (stdio closed)");
                        break Ok(());
                    };
                    if let Err(error) = remote.send(local_message).await {
                        break Err(error.into());
                    }
                }
                remote_message = remote.receive() => {
                    let Some(remote_message) = remote_message else {
                        info!("MCP bridge shutting down (NATS transport closed)");
                        break Ok(());
                    };
                    if let Err(error) = local.send(remote_message).await {
                        break Err(error.into());
                    }
                }
                _ = &mut shutdown_signal => {
                    info!("MCP bridge shutting down (signal received)");
                    break Ok(());
                }
            }
        }
    };

    if let Err(error) = local.close().await {
        error!(error = %error, "Failed to close stdio MCP transport");
    }
    if let Err(error) = remote.close().await {
        error!(error = %error, "Failed to close NATS MCP transport");
    }

    result
}

#[cfg(test)]
mod tests;
