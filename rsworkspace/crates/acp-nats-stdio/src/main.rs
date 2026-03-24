#![cfg_attr(coverage, allow(dead_code, unused_imports))]
mod config;

use acp_nats::{StdJsonSerialize, agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, SessionNotification};
use std::rc::Rc;
use tracing::{error, info};
use trogon_std::time::SystemClock;

#[cfg(not(coverage))]
use {
    acp_nats::nats, acp_telemetry::ServiceName, trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::base_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;
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

    let stdin = async_compat::Compat::new(tokio::io::stdin());
    let stdout = async_compat::Compat::new(tokio::io::stdout());

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(run_bridge(
            nats_client,
            &config,
            stdout,
            stdin,
            acp_telemetry::signal::shutdown_signal(),
        ))
        .await;

    if let Err(ref e) = result {
        error!(error = %e, "ACP bridge stopped with error");
    } else {
        info!("ACP bridge stopped");
    }

    acp_telemetry::shutdown_otel();

    result
}

#[cfg(coverage)]
fn main() {}

async fn run_bridge<N, W, R>(
    nats_client: N,
    config: &acp_nats::Config,
    stdout: W,
    stdin: R,
    shutdown_signal: impl std::future::Future<Output = ()>,
) -> Result<(), Box<dyn std::error::Error>>
where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + 'static,
    W: futures::AsyncWrite + Unpin + 'static,
    R: futures::AsyncRead + Unpin + 'static,
{
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
mod tests {
    use super::*;
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn run_bridge_shuts_down_on_signal() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let config = acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let (reader, _writer) = tokio::io::duplex(1024);
        let (_reader2, writer2) = tokio::io::duplex(1024);
        let stdin = async_compat::Compat::new(reader);
        let stdout = async_compat::Compat::new(writer2);

        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(run_bridge(
                mock,
                &config,
                stdout,
                stdin,
                std::future::ready(()),
            ))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_bridge_exits_on_io_close() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let config = acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let (reader, writer) = tokio::io::duplex(1024);
        let (_reader2, writer2) = tokio::io::duplex(1024);
        drop(writer);
        let stdin = async_compat::Compat::new(reader);
        let stdout = async_compat::Compat::new(writer2);

        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(run_bridge(
                mock,
                &config,
                stdout,
                stdin,
                std::future::pending(),
            ))
            .await;

        assert!(result.is_ok());
    }
}
