#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(coverage, allow(dead_code, unused_imports))]
mod config;

use acp_nats::boundary::{AbortOnDrop, BoundaryExit, ConnectionClient, connect_agent_boundary};
use acp_nats::{agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::schema::v1::SessionNotification;
use std::rc::Rc;
use std::sync::Arc;
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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
#[cfg_attr(coverage, coverage(off))]
fn main() {}

async fn run_bridge<N, J, W, R>(
    nats_client: N,
    js_client: J,
    config: &acp_nats::Config,
    stdout: W,
    stdin: R,
    shutdown_signal: impl std::future::Future<Output = ()>,
) -> Result<(), Box<dyn std::error::Error>>
where
    N: acp_nats::RequestClient + acp_nats::PublishClient + acp_nats::FlushClient + acp_nats::SubscribeClient + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
    W: futures::AsyncWrite + Send + Unpin + 'static,
    R: futures::AsyncRead + Send + Unpin + 'static,
{
    let meter = trogon_telemetry::meter("acp-io-bridge-nats");
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Arc::new(Bridge::new(
        nats_client.clone(),
        js_client,
        SystemClock,
        &meter,
        config.clone(),
        notification_tx,
    ));

    let boundary_result = connect_agent_boundary(bridge.clone(), stdout, stdin, async move |cx| {
        let _forwarder_guard = AbortOnDrop::new(spawn_notification_forwarder(
            ConnectionClient::new(cx.clone()),
            notification_rx,
        ));

        let mut client_task = AbortOnDrop::new(tokio::task::spawn_local(client::run(
            nats_client,
            Rc::new(ConnectionClient::new(cx)),
            bridge,
        )));
        info!("ACP bridge running on stdio with NATS client proxy");

        let shutdown_result = tokio::select! {
            result = client_task.handle_mut() => {
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
            _ = shutdown_signal => {
                info!("ACP bridge shutting down (signal received)");
                Ok(())
            }
        };

        if !client_task.is_finished() {
            client_task.abort_and_wait().await;
        }

        Ok(shutdown_result)
    })
    .await;

    match boundary_result? {
        BoundaryExit::Main(result) => result,
        BoundaryExit::TransportClosed => {
            info!("ACP bridge stdin closed; shutting down");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
