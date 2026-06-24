#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod config;

use acp_nats::{StdJsonSerialize, agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, SessionNotification};
use std::rc::Rc;
use tracing::{error, info};
use trogon_std::time::SystemClock;

#[cfg(not(coverage))]
use {
    acp_nats::nats,
    trogon_std::{env::SystemEnv, fs::SystemFs, signal::shutdown_signal},
    trogon_telemetry::{ResourceAttribute, ServiceName},
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::base_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;
    trogon_telemetry::init_logger(
        ServiceName::AcpNatsStdio,
        [ResourceAttribute::acp_prefix(config.acp_prefix())],
        &SystemEnv,
        &SystemFs,
    );
    let config = acp_nats::apply_timeout_overrides(config, &SystemEnv);

    info!("ACP bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(config.nats(), nats_connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(js_context);

    let stdin = async_compat::Compat::new(tokio::io::stdin());
    let stdout = async_compat::Compat::new(tokio::io::stdout());

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(run_bridge(
            nats_client,
            js_client,
            &config,
            stdout,
            stdin,
            shutdown_signal(),
        ))
        .await;

    if let Err(ref e) = result {
        error!(error = %e, "ACP bridge stopped with error");
    } else {
        info!("ACP bridge stopped");
    }

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result
}

#[cfg(coverage)]
fn main() {}

async fn run_bridge<N, J, W, R>(
    nats_client: N,
    js_client: J,
    config: &acp_nats::Config,
    stdout: W,
    stdin: R,
    shutdown_signal: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()>
where
    N: acp_nats::RequestClient + acp_nats::PublishClient + acp_nats::FlushClient + acp_nats::SubscribeClient + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
    W: futures::AsyncWrite + Unpin + 'static,
    R: futures::AsyncRead + Unpin + 'static,
{
    let meter = trogon_telemetry::meter("acp-io-bridge-nats");
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        js_client,
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
                    Err(e.into())
                }
            }
        }
        result = io_task => {
            match result {
                Err(e) => {
                    error!(error = %e, "IO task error");
                    Err(e.into())
                }
                Ok(()) => {
                    info!("ACP bridge shutting down (IO closed)");
                    Ok(())
                }
            }
        }
        _ = shutdown_signal => {
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

#[cfg(test)]
mod tests;
