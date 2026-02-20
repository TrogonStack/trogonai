mod config;
mod signal;
mod telemetry;

use acp_nats::{agent::Bridge, client, nats};
use agent_client_protocol::AgentSideConnection;
use async_nats::Client as NatsAsyncClient;
use std::rc::Rc;
use tracing::{error, info, warn};
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::from_env_with_provider(&SystemEnv);
    telemetry::init_logger(&config, &SystemEnv)?;

    info!("ACP bridge starting");

    let nats_client = match nats::connect(&config.nats).await {
        Ok(client) => Some(client),
        Err(e) => {
            warn!(error = %e, "Failed to connect to NATS");
            None
        }
    };
    let local = tokio::task::LocalSet::new();

    local.run_until(run_bridge(nats_client, &config)).await;

    info!("Flushing telemetry...");
    telemetry::shutdown_otel().await;
    info!("ACP bridge stopped");

    Ok(())
}

async fn run_bridge(nats_client: Option<NatsAsyncClient>, config: &acp_nats::Config) {
    let stdin = async_compat::Compat::new(tokio::io::stdin());
    let stdout = async_compat::Compat::new(tokio::io::stdout());

    // `Rc` is intentional: the ACP `Agent` trait is `?Send`, so the entire
    // bridge runs on a `LocalSet` with `spawn_local`. Do not replace with `Arc`
    // or move tasks to `tokio::spawn` — that would violate the `!Send` constraint.
    let bridge = Rc::new(Bridge::<NatsAsyncClient>::new(
        nats_client.clone(),
        config.acp_prefix.clone(),
    ));

    let (connection, io_task) = AgentSideConnection::new(bridge.clone(), stdout, stdin, |fut| {
        tokio::task::spawn_local(fut);
    });

    let connection = Rc::new(connection);

    if let Some(nats_instance) = nats_client {
        let client_connection = connection.clone();
        let bridge_for_client = bridge.clone();
        tokio::task::spawn_local(async move {
            client::run::<NatsAsyncClient, _>(nats_instance, client_connection, bridge_for_client)
                .await;
        });
        info!("ACP bridge running on stdio with NATS client proxy");
    } else {
        info!("ACP bridge running on stdio (no NATS)");
    }

    tokio::select! {
        result = io_task => {
            if let Err(e) = result {
                error!(error = %e, "IO task error");
            }
            info!("ACP bridge shutting down (IO closed)");
        }
        _ = signal::shutdown_signal() => {
            info!("ACP bridge shutting down (signal received)");
        }
    }
}
