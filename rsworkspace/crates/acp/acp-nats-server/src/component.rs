//! Bridge-backed ACP component driven by the SDK's HTTP/WebSocket transport.
//!
//! [`AcpHttpServer`](agent_client_protocol_http::AcpHttpServer) owns the remote
//! transport mechanics and asks a factory for one [`ConnectTo<Client>`] per
//! connection. This module is that factory's product: it stands up a per-connection
//! [`Bridge`] over NATS and hands the SDK-supplied transport straight to
//! [`connect_agent_boundary_with`], so the NATS leg keeps the subject-routed,
//! JetStream-durable design ADR#0020 preserves while the byte-stream boundary
//! becomes upstream's problem.

use acp_nats::boundary::{AbortOnDrop, ConnectionClient, connect_agent_boundary_with};
use acp_nats::{agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::{Agent, Client, ConnectTo, Result};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use trogon_std::time::SystemClock;

/// Matches the capacity the hand-rolled transport used for the same channel.
const NOTIFICATION_CHANNEL_CAPACITY: usize = 64;

/// One ACP connection's worth of bridge, awaiting a transport to drive it.
pub struct NatsAgentComponent<N, J> {
    nats: N,
    js: J,
    config: acp_nats::Config,
    shutdown_rx: watch::Receiver<bool>,
}

impl<N, J> NatsAgentComponent<N, J> {
    pub const fn new(nats: N, js: J, config: acp_nats::Config, shutdown_rx: watch::Receiver<bool>) -> Self {
        Self {
            nats,
            js,
            config,
            shutdown_rx,
        }
    }
}

impl<N, J> ConnectTo<Client> for NatsAgentComponent<N, J>
where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + Sync
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + Send + Sync + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    async fn connect_to(self, transport: impl ConnectTo<Agent>) -> Result<()> {
        let Self {
            nats,
            js,
            config,
            mut shutdown_rx,
        } = self;

        let meter = trogon_telemetry::meter("acp-nats-server");
        let (notification_tx, notification_rx) = mpsc::channel::<SessionNotification>(NOTIFICATION_CHANNEL_CAPACITY);
        let bridge = Arc::new(Bridge::new(
            nats.clone(),
            js,
            SystemClock,
            &meter,
            config,
            notification_tx,
        ));

        info!("ACP connection established");

        let outcome = connect_agent_boundary_with(bridge.clone(), transport, {
            let bridge = bridge.clone();
            async move |cx| {
                // Agent-to-client traffic reaches the peer through the SDK
                // connection handle; the NATS side never addresses it directly.
                let _forwarder_guard = AbortOnDrop::new(spawn_notification_forwarder(
                    ConnectionClient::new(cx.clone()),
                    notification_rx,
                ));

                let mut client_task = AbortOnDrop::new(tokio::spawn(client::run(
                    nats,
                    Arc::new(ConnectionClient::new(cx)),
                    bridge,
                )));

                // Hold the connection open until the peer disconnects (handled by
                // `connect_agent_boundary_with`), the client proxy stops, or the
                // process is draining. Returning here tears the connection down.
                tokio::select! {
                    _ = client_task.handle_mut() => {}
                    _ = shutdown_rx.wait_for(|draining| *draining) => {
                        info!("Draining ACP connection for shutdown");
                    }
                }

                if !client_task.is_finished() {
                    client_task.abort_and_wait().await;
                }
                Ok(())
            }
        })
        .await;

        // Session-ready publishes and other bridge-owned background work outlive
        // the dispatch loop; drain them before the connection's NATS client drops.
        bridge.drain_background_tasks().await;

        match outcome {
            Ok(_) => {
                info!("ACP connection closed");
                Ok(())
            }
            Err(error) => {
                warn!(error = %error, "ACP connection closed with error");
                Err(error)
            }
        }
    }
}
