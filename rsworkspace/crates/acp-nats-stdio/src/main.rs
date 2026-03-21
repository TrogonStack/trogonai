mod config;

use acp_nats::{StdJsonSerialize, agent::Bridge, client, nats, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, SessionNotification};
use async_nats::Client as NatsAsyncClient;
use std::rc::Rc;
use tracing::{error, info};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;
use trogon_std::time::SystemClock;

use acp_telemetry::ServiceName;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::base_config(&SystemEnv)?;
    acp_telemetry::init_logger(
        ServiceName::AcpNatsStdio,
        config.acp_prefix(),
        &SystemEnv,
        &SystemFs,
    );
    let config = acp_nats::apply_timeout_overrides(config, &SystemEnv);

    info!("ACP bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(config.nats(), nats_connect_timeout).await?;

    let local = tokio::task::LocalSet::new();

    let result = local.run_until(run_bridge(nats_client, &config)).await;

    if let Err(ref e) = result {
        error!(error = %e, "ACP bridge stopped with error");
    } else {
        info!("ACP bridge stopped");
    }

    acp_telemetry::shutdown_otel();

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

    let meter = acp_telemetry::meter("acp-io-bridge-nats");
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        SystemClock,
        &meter,
        config.clone(),
        notification_tx,
    ));

    let (connection, io_task) = AgentSideConnection::new(bridge.clone(), stdout, stdin, |fut| {
        tokio::task::spawn_local(fut);
    });

    let connection = Rc::new(connection);

    spawn_notification_forwarder(connection.clone(), notification_rx);

    let client_connection = connection.clone();
    let bridge_for_client = bridge.clone();
    let mut client_task = tokio::task::spawn_local(client::run(
        nats_client,
        client_connection,
        bridge_for_client,
        StdJsonSerialize,
    ));
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
        _ = acp_telemetry::signal::shutdown_signal() => {
            info!("ACP bridge shutting down (signal received)");
            Ok(())
        }
    };

    if !client_task.is_finished() {
        client_task.abort();
        let _ = client_task.await;
    }

    shutdown_result
}
