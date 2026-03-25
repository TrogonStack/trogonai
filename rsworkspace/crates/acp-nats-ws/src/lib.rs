#![cfg_attr(coverage, feature(coverage_attribute))]
pub mod config;
pub mod connection;
pub mod upgrade;

use tokio::sync::mpsc;
use tracing::info;
use upgrade::ConnectionRequest;

pub const THREAD_NAME: &str = "acp-ws-local";

/// Spawns the connection thread and returns its `JoinHandle`.
///
/// The thread runs a single-threaded tokio runtime with a `LocalSet`. All
/// WebSocket connections live here because the ACP `Agent` trait is `?Send`,
/// requiring `spawn_local` / `Rc`.
#[cfg_attr(coverage, coverage(off))]
pub fn start_connection_thread<N>(
    conn_rx: mpsc::UnboundedReceiver<ConnectionRequest>,
    nats_client: N,
    config: acp_nats::Config,
) -> std::thread::JoinHandle<()>
where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
{
    std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || run_connection_thread(conn_rx, nats_client, config))
        .expect("failed to spawn connection thread")
}

/// Runs a single-threaded tokio runtime with a `LocalSet`. All WebSocket
/// connections are processed here because the ACP `Agent` trait is `?Send`,
/// requiring `spawn_local` / `Rc`.
#[cfg_attr(coverage, coverage(off))]
pub fn run_connection_thread<N>(
    conn_rx: mpsc::UnboundedReceiver<ConnectionRequest>,
    nats_client: N,
    config: acp_nats::Config,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create per-connection runtime");

    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(process_connections(conn_rx, nats_client, config)));

    // run_until returns once its future completes, but sub-tasks
    // spawned by connection handlers (pumps, AgentSideConnection
    // internals) may still be live on the LocalSet. Drive them to
    // completion so WebSocket close frames are sent and per-connection
    // cleanup finishes.
    rt.block_on(local);
    info!("Local thread exiting");
}

#[cfg_attr(coverage, coverage(off))]
async fn process_connections<N>(
    mut conn_rx: mpsc::UnboundedReceiver<ConnectionRequest>,
    nats_client: N,
    config: acp_nats::Config,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
{
    let mut conn_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    while let Some(req) = conn_rx.recv().await {
        conn_handles.retain(|h| !h.is_finished());
        let client = nats_client.clone();
        let cfg = config.clone();
        conn_handles.push(tokio::task::spawn_local(connection::handle(
            req.socket,
            client,
            cfg,
            req.shutdown_rx,
        )));
    }

    let active = conn_handles.iter().filter(|h| !h.is_finished()).count();
    info!(
        active_connections = active,
        "Connection channel closed, draining active connections"
    );

    for handle in conn_handles {
        let _ = handle.await;
    }

    info!("All connections drained");
}
