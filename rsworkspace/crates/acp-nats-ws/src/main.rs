#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(coverage, allow(dead_code, unused_imports))]
mod config;
mod connection;
mod constants;
mod upgrade;

use tokio::sync::{mpsc, watch};
use tracing::info;
use upgrade::{ConnectionRequest, UpgradeState};

#[cfg(not(coverage))]
use {
    acp_nats::nats, acp_telemetry::ServiceName, clap::Parser, std::net::SocketAddr,
    tower_http::trace::TraceLayer, tracing::error, trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = config::Args::parse();
    let ws_config = config::config_from_args(args, &SystemEnv)?;
    acp_telemetry::init_logger(
        ServiceName::AcpNatsWs,
        ws_config.acp.acp_prefix(),
        &SystemEnv,
        &SystemFs,
    );
    let ws_config = config::apply_timeout_overrides(ws_config, &SystemEnv);

    info!("ACP WebSocket bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(ws_config.acp.nats(), nats_connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(js_context);

    let (shutdown_tx, _) = watch::channel(false);
    let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

    let conn_thread = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || run_connection_thread(conn_rx, nats_client, js_client, ws_config.acp))?;

    let state = UpgradeState {
        conn_tx,
        shutdown_tx: shutdown_tx.clone(),
    };

    let app = axum::Router::new()
        .route("/ws", axum::routing::get(upgrade::handle))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from((ws_config.host, ws_config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(address = %addr, "Listening for WebSocket connections");

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            acp_telemetry::signal::shutdown_signal().await;
            info!("Shutdown signal received, stopping server");
            let _ = shutdown_tx.send(true);
        })
        .await;

    match &result {
        Ok(()) => info!("ACP WebSocket bridge stopped"),
        Err(e) => error!(error = %e, "ACP WebSocket bridge stopped with error"),
    }

    // `serve` returning drops the Router (and its AppState.conn_tx), which
    // closes the channel and lets the connection thread's recv-loop exit and
    // drain active connections. Wait for that drain to finish before tearing
    // down telemetry.
    if let Err(e) = conn_thread.join() {
        error!("Connection thread panicked: {e:?}");
    }

    acp_telemetry::shutdown_otel();

    result.map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(coverage)]
fn main() {}

use constants::THREAD_NAME;

/// Runs a single-threaded tokio runtime with a
/// `LocalSet`. All WebSocket connections are processed here because the ACP
/// `Agent` trait is `?Send`, requiring `spawn_local` / `Rc`.
fn run_connection_thread<N, J>(
    conn_rx: mpsc::UnboundedReceiver<ConnectionRequest>,
    nats_client: N,
    js_client: J,
    config: acp_nats::Config,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + Send + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to create per-connection runtime");

    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(process_connections(conn_rx, nats_client, js_client, config)));

    // run_until returns once process_connections completes, but
    // sub-tasks spawned by connection handlers (pumps,
    // AgentSideConnection internals) may still be live on the
    // LocalSet. Drive them for a bounded window so WebSocket close
    // frames are sent and per-connection cleanup finishes, without
    // blocking forever on stuck sub-tasks (e.g. a hanging NATS
    // request that never resolves).
    let drain = std::time::Duration::from_secs(5);
    rt.block_on(local.run_until(async { tokio::time::sleep(drain).await }));
    info!("Local thread exiting");
}

async fn process_connections<N, J>(
    mut conn_rx: mpsc::UnboundedReceiver<ConnectionRequest>,
    nats_client: N,
    js_client: J,
    config: acp_nats::Config,
) where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + Clone
        + Send
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
{
    let mut conn_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    while let Some(req) = conn_rx.recv().await {
        conn_handles.retain(|h| !h.is_finished());
        let client = nats_client.clone();
        let js = js_client.clone();
        let cfg = config.clone();
        conn_handles.push(tokio::task::spawn_local(connection::handle(
            req.socket,
            client,
            js,
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

#[cfg(test)]
mod tests {
    use acp_nats::Config;
    use acp_nats_ws::upgrade::{ConnectionRequest, UpgradeState};
    use acp_nats_ws::{THREAD_NAME, run_connection_thread, upgrade};
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, watch};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use trogon_nats::AdvancedMockNatsClient;

    #[derive(Clone)]
    struct MockJs {
        publisher: trogon_nats::jetstream::MockJetStreamPublisher,
        consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory,
    }

    impl MockJs {
        fn new() -> Self {
            Self {
                publisher: trogon_nats::jetstream::MockJetStreamPublisher::new(),
                consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory::new(),
            }
        }
    }

    impl trogon_nats::jetstream::JetStreamPublisher for MockJs {
        type PublishError = trogon_nats::mocks::MockError;
        type AckFuture = std::future::Ready<
            Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>,
        >;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: async_nats::HeaderMap,
            payload: bytes::Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            self.publisher
                .publish_with_headers(subject, headers, payload)
                .await
        }
    }

    impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
        type Error = trogon_nats::mocks::MockError;
        type Stream = trogon_nats::jetstream::MockJetStreamStream;

        async fn get_stream<T: AsRef<str> + Send>(
            &self,
            stream_name: T,
        ) -> Result<trogon_nats::jetstream::MockJetStreamStream, Self::Error> {
            self.consumer_factory.get_stream(stream_name).await
        }
    }

    #[tokio::test]
    async fn test_websocket_connection_lifecycle() {
        let nats_mock = AdvancedMockNatsClient::new();
        let config = Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        // Required by AdvancedMockNatsClient to not error out on subscribe()
        let _injector = nats_mock.inject_messages();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

        let nats_mock_clone = nats_mock.clone();
        let conn_thread = std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || run_connection_thread(conn_rx, nats_mock_clone, MockJs::new(), config))
            .expect("failed to spawn connection thread");

        let state = UpgradeState {
            conn_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(upgrade::handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
                .unwrap();
        });

        // Setup mock response for NATS
        let nats_response = r#"{"agentCapabilities": {"loadSession": false, "mcpCapabilities": {"http": false, "sse": false}, "promptCapabilities": {"audio": false, "embeddedContext": false, "image": false}, "sessionCapabilities": {}}, "authMethods": [], "protocolVersion": 0}"#;
        nats_mock.set_response("acp.agent.initialize", nats_response.into());

        // Connect client
        let ws_url = format!("ws://{}/ws", addr);
        let (mut ws_stream, _) = connect_async(ws_url).await.unwrap();

        // Send initialize request
        let req =
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": 0}}"#;
        ws_stream.send(Message::Text(req.into())).await.unwrap();

        // Await response
        let msg = tokio::time::timeout(Duration::from_secs(2), ws_stream.next())
            .await
            .expect("timeout waiting for response")
            .expect("stream closed")
            .unwrap();

        let expected_ws_response = r#"{"id":1,"jsonrpc":"2.0","result":{"agentCapabilities":{"auth":{},"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}}"#;

        let text = msg.to_text().expect("Expected text message").to_string();
        // order of fields in JSON might vary, so we parse to compare
        let actual: serde_json::Value = serde_json::from_str(&text).unwrap();
        let expected: serde_json::Value = serde_json::from_str(expected_ws_response).unwrap();
        assert_eq!(actual, expected);

        // Trigger shutdown
        shutdown_tx.send(true).unwrap();

        // Ensure clean teardown
        let _ = tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task did not shut down");

        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn test_shutdown_while_connection_active() {
        let nats_mock = AdvancedMockNatsClient::new();
        let config = Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let _injector = nats_mock.inject_messages();

        nats_mock.hang_next_request();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

        let nats_mock_clone = nats_mock.clone();
        let conn_thread = std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || run_connection_thread(conn_rx, nats_mock_clone, MockJs::new(), config))
            .expect("failed to spawn connection thread");

        let state = UpgradeState {
            conn_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(upgrade::handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
                .unwrap();
        });

        let ws_url = format!("ws://{}/ws", addr);
        let (mut ws_stream, _) = connect_async(&ws_url).await.unwrap();

        let req =
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": 0}}"#;
        ws_stream.send(Message::Text(req.into())).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        shutdown_tx.send(true).unwrap();

        let _ = tokio::time::timeout(Duration::from_secs(5), server_task)
            .await
            .expect("server task did not shut down");

        drop(ws_stream);

        conn_thread.join().unwrap();
    }

    /// Sends a binary frame with invalid UTF-8 bytes — exercises the `Err(e) => warn!` path
    /// in run_recv_pump (connection.rs lines 161-166). The pump logs a warning and continues;
    /// the connection must not panic or crash.
    #[tokio::test]
    async fn test_recv_pump_drops_non_utf8_frame_and_continues() {
        let nats_mock = AdvancedMockNatsClient::new();
        let config = Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );
        let _injector = nats_mock.inject_messages();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();

        let nats_mock_clone = nats_mock.clone();
        let conn_thread = std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || run_connection_thread(conn_rx, nats_mock_clone, config))
            .unwrap();

        let state = UpgradeState {
            conn_tx,
            shutdown_tx: shutdown_tx.clone(),
        };

        let app = axum::Router::new()
            .route("/ws", axum::routing::get(upgrade::handle))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
                .unwrap();
        });

        let ws_url = format!("ws://{}/ws", addr);
        let (mut ws_stream, _) = connect_async(ws_url).await.unwrap();

        // Invalid UTF-8 sequence — exercises the warn path in run_recv_pump
        let invalid_utf8: Vec<u8> = vec![0xFF, 0xFE, 0x80, 0x00];
        ws_stream
            .send(Message::Binary(invalid_utf8.into()))
            .await
            .unwrap();

        // Pump continues; give it a moment then shut down cleanly
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown_tx.send(true).unwrap();

        let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
        conn_thread.join().unwrap();
    }

    /// `start_connection_thread` spawns a thread and returns a JoinHandle that
    /// exits cleanly when the connection channel is closed.
    #[tokio::test]
    async fn test_start_connection_thread_exits_cleanly_when_channel_closed() {
        use acp_nats_ws::start_connection_thread;

        let nats_mock = AdvancedMockNatsClient::new();
        let config = Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let (conn_tx, conn_rx) = mpsc::unbounded_channel::<ConnectionRequest>();
        let handle = start_connection_thread(conn_rx, nats_mock, config);

        drop(conn_tx);

        let result = tokio::task::spawn_blocking(move || handle.join())
            .await
            .unwrap();
        assert!(
            result.is_ok(),
            "start_connection_thread handle must join cleanly"
        );
    }
}
