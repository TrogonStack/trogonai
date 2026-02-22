mod config;
mod signal;
mod telemetry;

use acp_nats::{agent::Bridge, client, nats};
use agent_client_protocol::AgentSideConnection;
use async_nats::Client as NatsAsyncClient;
use std::rc::Rc;
use tracing::{error, info};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;
use trogon_std::time::SystemClock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::from_env_with_provider(&SystemEnv)?;
    telemetry::init_logger(config.acp_prefix(), &SystemEnv, &SystemFs);

    info!("ACP bridge starting");

    let nats_connect_timeout = config::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(config.nats(), nats_connect_timeout).await?;

    let local = tokio::task::LocalSet::new();

    let result = local.run_until(run_bridge(nats_client, &config)).await;

    telemetry::shutdown_otel();

    if let Err(ref e) = result {
        error!(error = %e, "ACP bridge stopped with error");
    } else {
        info!("ACP bridge stopped");
    }

    result
}

// `Rc` is intentional throughout this function: the ACP `Agent` trait is
// `?Send`, so the entire bridge runs on a `LocalSet` with `spawn_local`.
// Do not replace with `Arc` or move tasks to `tokio::spawn` — that would
// violate the `!Send` constraint.
async fn run_bridge(
    nats_client: NatsAsyncClient,
    config: &acp_nats::Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = async_compat::Compat::new(tokio::io::stdin());
    let stdout = async_compat::Compat::new(tokio::io::stdout());

    let meter = opentelemetry::global::meter("acp-io-bridge-nats");
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        SystemClock,
        &meter,
        config.clone(),
    ));

    let (connection, io_task) = AgentSideConnection::new(bridge.clone(), stdout, stdin, |fut| {
        tokio::task::spawn_local(fut);
    });

    let connection = Rc::new(connection);

    let client_connection = connection.clone();
    let bridge_for_client = bridge.clone();
    let mut client_task = tokio::task::spawn_local(async move {
        client::run(nats_client, client_connection, bridge_for_client).await;
    });
    info!("ACP bridge running on stdio with NATS client proxy");

    let shutdown_result = tokio::select! {
        result = &mut client_task => {
            match result {
                Ok(()) => {
                    info!("ACP bridge client task completed");
                    Ok(())
                }
                Err(e) => {
                    error!(error = %e, "Client task ended with error");
                    Err(Box::new(e) as Box<dyn std::error::Error>)
                }
            }
        }
        result = io_task => {
            match result {
                Err(e) => {
                    error!(error = %e, "IO task error");
                    Err(Box::new(e) as Box<dyn std::error::Error>)
                }
                Ok(()) => {
                    info!("ACP bridge shutting down (IO closed)");
                    Ok(())
                }
            }
        }
        _ = signal::shutdown_signal() => {
            info!("ACP bridge shutting down (signal received)");
            Ok(())
        }
    };

    if !client_task.is_finished() {
        // Client task is explicitly aborted on shutdown to avoid running past stdio lifecycle.
        client_task.abort();
    }
    let _ = client_task.await;
    bridge.await_session_ready_tasks().await;

    shutdown_result
}
