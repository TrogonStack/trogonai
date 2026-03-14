mod config;
mod connection;
mod signal;
mod telemetry;

use acp_nats::nats;
use async_nats::Client as NatsAsyncClient;
use axum::extract::State;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::any;
use std::net::SocketAddr;
use tokio::sync::{broadcast, mpsc};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;

struct ConnectionRequest {
    socket: WebSocket,
    shutdown_rx: broadcast::Receiver<()>,
}

#[derive(Clone)]
struct AppState {
    conn_tx: mpsc::UnboundedSender<ConnectionRequest>,
    shutdown_tx: broadcast::Sender<()>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ws_config = config::base_config(&SystemEnv)?;
    telemetry::init_logger(ws_config.acp.acp_prefix(), &SystemEnv, &SystemFs);
    let ws_config = config::apply_timeout_overrides(ws_config, &SystemEnv);

    info!("ACP WebSocket bridge starting");

    let nats_connect_timeout = config::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(ws_config.acp.nats(), nats_connect_timeout).await?;

    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

    spawn_connection_thread(conn_rx, nats_client, ws_config.acp);

    let state = AppState {
        conn_tx,
        shutdown_tx: shutdown_tx.clone(),
    };

    let app = axum::Router::new()
        .route("/ws", any(ws_upgrade_handler))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from((ws_config.host, ws_config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(address = %addr, "Listening for WebSocket connections");

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            signal::shutdown_signal().await;
            info!("Shutdown signal received, stopping server");
            let _ = shutdown_tx.send(());
        })
        .await;

    match &result {
        Ok(()) => info!("ACP WebSocket bridge stopped"),
        Err(e) => error!(error = %e, "ACP WebSocket bridge stopped with error"),
    }

    telemetry::shutdown_otel();

    result.map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

async fn ws_upgrade_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let shutdown_rx = state.shutdown_tx.subscribe();
    ws.on_upgrade(move |socket| async move {
        if state
            .conn_tx
            .send(ConnectionRequest {
                socket,
                shutdown_rx,
            })
            .is_err()
        {
            error!("Connection thread is gone; dropping WebSocket");
        }
    })
}

/// Spawns a dedicated OS thread running a single-threaded tokio runtime with a
/// `LocalSet`. All WebSocket connections are processed here because the ACP
/// `Agent` trait is `?Send`, requiring `spawn_local` / `Rc`.
fn spawn_connection_thread(
    mut conn_rx: mpsc::UnboundedReceiver<ConnectionRequest>,
    nats_client: NatsAsyncClient,
    config: acp_nats::Config,
) {
    std::thread::Builder::new()
        .name("acp-ws-local".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to create per-connection runtime");

            let local = tokio::task::LocalSet::new();
            rt.block_on(local.run_until(async move {
                while let Some(req) = conn_rx.recv().await {
                    let client = nats_client.clone();
                    let cfg = config.clone();
                    tokio::task::spawn_local(async move {
                        connection::handle_connection(req.socket, client, cfg, req.shutdown_rx)
                            .await;
                    });
                }
                info!("Connection channel closed, local thread exiting");
            }));
        })
        .expect("failed to spawn connection thread");
}
