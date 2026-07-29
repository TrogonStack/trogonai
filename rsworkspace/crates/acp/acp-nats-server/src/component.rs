//! Bridge-backed ACP component driven by the SDK's HTTP/WebSocket transport.
//!
//! [`AcpHttpServer`](agent_client_protocol_http::AcpHttpServer) owns the remote
//! transport mechanics and asks a factory for one [`ConnectTo<Client>`] per
//! connection. This module is that factory's product: it stands up a per-connection
//! [`Bridge`] over NATS and hands the SDK-supplied transport straight to
//! [`connect_agent_boundary_with`], so the NATS leg keeps the subject-routed,
//! JetStream-durable design ADR#0020 preserves while the byte-stream boundary
//! becomes upstream's problem.

use acp_nats::boundary::{AbortOnDrop, BoundaryExit, ConnectionClient, connect_agent_boundary_with};
use acp_nats::{agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::schema::v1::SessionNotification;
use agent_client_protocol::{Agent, Client, ConnectTo, Result};
use opentelemetry::metrics::Meter;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};
use trogon_std::time::SystemClock;

/// Matches the capacity the hand-rolled transport used for the same channel.
const NOTIFICATION_CHANNEL_CAPACITY: usize = 64;

/// Maps the client proxy's join result onto the connection's outcome.
///
/// A `JoinError` means the proxy panicked or was cancelled out from under us, so
/// the connection did not close cleanly. Reporting `Ok` there would tell the SDK
/// transport the peer disconnected normally and lose the failure, which is what
/// the WebSocket and stdio paths avoid by inspecting this result.
fn client_task_outcome(result: std::result::Result<(), tokio::task::JoinError>) -> Result<()> {
    match result {
        Ok(()) => {
            info!("Client proxy stopped");
            Ok(())
        }
        Err(source) => {
            error!(error = %source, "Client proxy ended with error");
            Err(agent_client_protocol::Error::into_internal_error(source))
        }
    }
}

/// One ACP connection's worth of bridge, awaiting a transport to drive it.
pub struct NatsAgentComponent<N, J> {
    nats: N,
    js: J,
    config: acp_nats::Config,
    meter: Meter,
    shutdown_rx: watch::Receiver<bool>,
}

impl<N, J> NatsAgentComponent<N, J> {
    /// The factory this is built from runs once per connection, so anything
    /// process-wide is a caller's responsibility rather than something to look up
    /// here. `meter` is that: the instrumentation scope is a property of the
    /// service, not of a connection, so it is created once and cloned in.
    pub const fn new(
        nats: N,
        js: J,
        config: acp_nats::Config,
        meter: Meter,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            nats,
            js,
            config,
            meter,
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
            meter,
            mut shutdown_rx,
        } = self;

        // Per-connection by necessity: this channel feeds notifications to *this*
        // connection's SDK handle, and the bridge owns per-connection state
        // (pending prompt waiters, background tasks) keyed to that sender.
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
                let outcome = tokio::select! {
                    result = client_task.handle_mut() => client_task_outcome(result),
                    _ = shutdown_rx.wait_for(|draining| *draining) => {
                        info!("Draining ACP connection for shutdown");
                        Ok(())
                    }
                };

                // No `is_finished` guard: `abort_and_wait` already no-ops on a
                // finished task, which is the property that keeps a caller from
                // re-awaiting an already-awaited handle.
                client_task.abort_and_wait().await;
                outcome
            }
        })
        .await;

        // Session-ready publishes and other bridge-owned background work outlive
        // the dispatch loop; drain them before the connection's NATS client drops.
        bridge.drain_background_tasks().await;

        connection_outcome(outcome)
    }
}

/// Collapses a boundary exit into the transport-facing connection result.
///
/// Both `BoundaryExit` variants are ordinary closes: the peer hung up, or the
/// bridge decided to stop. Only a boundary error is a failure the transport
/// needs to see.
fn connection_outcome(outcome: Result<BoundaryExit<()>>) -> Result<()> {
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

#[cfg(test)]
mod tests;
