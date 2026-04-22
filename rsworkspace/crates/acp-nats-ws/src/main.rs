mod acp_connection_id;
mod config;
mod connection;
mod constants;
mod transport;

use tokio::sync::{mpsc, watch};
use tracing::info;
use transport::{AppState, ManagerRequest};

#[cfg(not(coverage))]
use {
    acp_nats::nats, acp_telemetry::ServiceName, clap::Parser, std::net::SocketAddr, tracing::error,
    trogon_std::env::SystemEnv, trogon_std::fs::SystemFs,
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

    info!("ACP remote transport bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(ws_config.acp.nats(), nats_connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(js_context);

    let (shutdown_tx, _) = watch::channel(false);
    let (manager_tx, manager_rx) = mpsc::unbounded_channel::<ManagerRequest>();

    let conn_thread = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || run_connection_thread(manager_rx, nats_client, js_client, ws_config.acp))?;

    let state = AppState {
        bind_host: ws_config.host,
        manager_tx,
        shutdown_tx: shutdown_tx.clone(),
    };

    let app = trogon_std::telemetry::http::instrument_router(
        axum::Router::new()
            .route(
                ACP_ENDPOINT,
                axum::routing::get(transport::get)
                    .post(transport::post)
                    .delete(transport::delete),
            )
            .with_state(state),
    );

    let addr = SocketAddr::from((ws_config.host, ws_config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(address = %addr, "Listening for ACP transport connections");

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            acp_telemetry::signal::shutdown_signal().await;
            info!("Shutdown signal received, stopping server");
            let _ = shutdown_tx.send(true);
        })
        .await;

    match &result {
        Ok(()) => info!("ACP remote transport bridge stopped"),
        Err(e) => error!(error = %e, "ACP remote transport bridge stopped with error"),
    }

    // `serve` returning drops the Router (and its AppState.conn_tx), which
    // closes the channel and lets the connection thread's recv-loop exit and
    // drain active connections. Wait for that drain to finish before tearing
    // down telemetry.
    if let Err(e) = conn_thread.join() {
        error!("Connection thread panicked: {e:?}");
    }

    if let Err(e) = acp_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result.map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(coverage)]
fn main() {}

use constants::{ACP_ENDPOINT, THREAD_NAME};

/// Runs a single-threaded tokio runtime with a
/// `LocalSet`. All WebSocket connections are processed here because the ACP
/// `Agent` trait is `?Send`, requiring `spawn_local` / `Rc`.
fn run_connection_thread<N, J>(
    manager_rx: mpsc::UnboundedReceiver<ManagerRequest>,
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
    rt.block_on(local.run_until(process_connections(
        manager_rx,
        nats_client,
        js_client,
        config,
    )));

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
    mut manager_rx: mpsc::UnboundedReceiver<ManagerRequest>,
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
    let mut websocket_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut http_connection_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut http_connections = std::collections::HashMap::new();

    while let Some(request) = manager_rx.recv().await {
        transport::process_manager_request(
            request,
            &mut http_connections,
            &mut websocket_handles,
            &mut http_connection_handles,
            &nats_client,
            &js_client,
            &config,
        )
        .await;
    }

    let active = websocket_handles
        .iter()
        .filter(|h| !h.is_finished())
        .count()
        + http_connection_handles
            .iter()
            .filter(|h| !h.is_finished())
            .count();
    info!(
        active_connections = active,
        "Connection channel closed, draining active connections"
    );

    for handle in websocket_handles {
        let _ = handle.await;
    }
    for handle in http_connection_handles {
        let _ = handle.await;
    }

    info!("All connections drained");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{
        ACP_CONNECTION_ID_HEADER, ACP_PROTOCOL_VERSION_HEADER, ACP_SESSION_ID_HEADER,
    };
    use acp_nats::Config;
    use axum::body::{Body, to_bytes};
    use axum::http::header::{ACCEPT, CONTENT_TYPE};
    use axum::http::{Request, StatusCode};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use tower::ServiceExt;
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

    fn test_config() -> Config {
        Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        )
    }

    fn test_app(state: AppState) -> axum::Router {
        axum::Router::new()
            .route(
                ACP_ENDPOINT,
                axum::routing::get(transport::get)
                    .post(transport::post)
                    .delete(transport::delete),
            )
            .with_state(state)
    }

    fn spawn_connection_thread(
        nats_mock: AdvancedMockNatsClient,
        manager_rx: mpsc::UnboundedReceiver<ManagerRequest>,
    ) -> std::thread::JoinHandle<()> {
        let config = test_config();
        std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || run_connection_thread(manager_rx, nats_mock, MockJs::new(), config))
            .expect("failed to spawn connection thread")
    }

    fn build_test_app(
        nats_mock: AdvancedMockNatsClient,
    ) -> (
        axum::Router,
        watch::Sender<bool>,
        std::thread::JoinHandle<()>,
    ) {
        let (shutdown_tx, _) = watch::channel(false);
        let (manager_tx, manager_rx) = mpsc::unbounded_channel::<ManagerRequest>();
        let conn_thread = spawn_connection_thread(nats_mock, manager_rx);
        let app = test_app(AppState {
            bind_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            manager_tx,
            shutdown_tx: shutdown_tx.clone(),
        });
        (app, shutdown_tx, conn_thread)
    }

    async fn start_test_server(
        nats_mock: AdvancedMockNatsClient,
    ) -> (
        std::net::SocketAddr,
        watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
        std::thread::JoinHandle<()>,
    ) {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let (manager_tx, manager_rx) = mpsc::unbounded_channel::<ManagerRequest>();
        let conn_thread = spawn_connection_thread(nats_mock, manager_rx);
        let app = test_app(AppState {
            bind_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            manager_tx,
            shutdown_tx: shutdown_tx.clone(),
        });

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

        (addr, shutdown_tx, server_task, conn_thread)
    }

    async fn body_text(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn sse_events(body: &str) -> Vec<Value> {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|json| !json.is_empty())
            .map(|json| serde_json::from_str(json).unwrap())
            .collect()
    }

    fn http_post_request(body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(ACP_ENDPOINT)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn test_websocket_connection_lifecycle() {
        let nats_mock = AdvancedMockNatsClient::new();

        // Required by AdvancedMockNatsClient to not error out on subscribe()
        let _injector = nats_mock.inject_messages();

        // Setup mock response for NATS
        let nats_response = r#"{"agentCapabilities": {"loadSession": false, "mcpCapabilities": {"http": false, "sse": false}, "promptCapabilities": {"audio": false, "embeddedContext": false, "image": false}, "sessionCapabilities": {}}, "authMethods": [], "protocolVersion": 0}"#;
        nats_mock.set_response("acp.agent.initialize", nats_response.into());

        let (addr, shutdown_tx, server_task, conn_thread) = start_test_server(nats_mock).await;

        // Connect client
        let ws_url = format!("ws://{}{}", addr, ACP_ENDPOINT);
        let (mut ws_stream, response) = connect_async(ws_url).await.unwrap();
        assert!(response.headers().contains_key(ACP_CONNECTION_ID_HEADER));

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

        match msg {
            Message::Text(t) => {
                let text = t.to_string();
                // order of fields in JSON might vary, so we parse to compare
                let actual: serde_json::Value = serde_json::from_str(&text).unwrap();
                let expected: serde_json::Value =
                    serde_json::from_str(expected_ws_response).unwrap();
                assert_eq!(actual, expected);
            }
            _ => panic!("Expected text message"),
        }

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

        let _injector = nats_mock.inject_messages();

        nats_mock.hang_next_request();
        let (addr, shutdown_tx, server_task, conn_thread) = start_test_server(nats_mock).await;

        let ws_url = format!("ws://{}{}", addr, ACP_ENDPOINT);
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

    #[tokio::test]
    async fn streamable_http_initialize_returns_connection_id_and_sse_response() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
                .into(),
        );

        let (app, shutdown_tx, conn_thread) = build_test_app(nats_mock);
        let response = app
            .clone()
            .oneshot(http_post_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "text/event-stream"
        );

        let connection_id = response
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(crate::acp_connection_id::AcpConnectionId::parse(&connection_id).is_ok());

        let body = body_text(response).await;
        let events = sse_events(&body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["id"], 1);
        assert_eq!(events[0]["result"]["protocolVersion"], 0);

        shutdown_tx.send(true).unwrap();
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_session_new_returns_session_header_and_body() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
                .into(),
        );
        nats_mock.set_response(
            "acp.agent.session.new",
            r#"{"sessionId":"test-session-1"}"#.into(),
        );

        let (app, shutdown_tx, conn_thread) = build_test_app(nats_mock);
        let initialize = app
            .clone()
            .oneshot(http_post_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#,
            ))
            .await
            .unwrap();
        let connection_id = initialize
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let _ = body_text(initialize).await;

        let session_new = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(ACP_ENDPOINT)
                    .header(CONTENT_TYPE, "application/json")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                    .header(ACP_PROTOCOL_VERSION_HEADER, "0")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":".","mcpServers":[]}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(session_new.status(), StatusCode::OK);
        assert_eq!(
            session_new
                .headers()
                .get(ACP_CONNECTION_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            connection_id
        );

        let session_id = session_new
            .headers()
            .get(ACP_SESSION_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        assert_eq!(
            session_new
                .headers()
                .get(ACP_PROTOCOL_VERSION_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("0")
        );
        let body = body_text(session_new).await;
        let events = sse_events(&body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["id"], 2);
        assert_eq!(events[0]["result"]["sessionId"], "test-session-1");
        assert_eq!(session_id.as_deref(), Some("test-session-1"));

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_get_requires_connection_and_session_headers() {
        let nats_mock = AdvancedMockNatsClient::new();
        let (app, shutdown_tx, conn_thread) = build_test_app(nats_mock);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(ACP_ENDPOINT)
                    .header(ACCEPT, "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn legacy_websocket_alias_is_not_routed() {
        let nats_mock = AdvancedMockNatsClient::new();
        let (app, shutdown_tx, conn_thread) = build_test_app(nats_mock);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/ws")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_delete_terminates_initialized_connection() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
                .into(),
        );

        let (app, shutdown_tx, conn_thread) = build_test_app(nats_mock);
        let initialize = app
            .clone()
            .oneshot(http_post_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#,
            ))
            .await
            .unwrap();
        let connection_id = initialize
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let _ = body_text(initialize).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(ACP_ENDPOINT)
                    .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                    .header(ACP_PROTOCOL_VERSION_HEADER, "0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            response
                .headers()
                .get(ACP_PROTOCOL_VERSION_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("0")
        );

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_rejects_mismatched_protocol_version_after_initialize() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
                .into(),
        );

        let (app, shutdown_tx, conn_thread) = build_test_app(nats_mock);
        let initialize = app
            .clone()
            .oneshot(http_post_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#,
            ))
            .await
            .unwrap();
        let connection_id = initialize
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let _ = body_text(initialize).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(ACP_ENDPOINT)
                    .header(CONTENT_TYPE, "application/json")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                    .header(ACP_PROTOCOL_VERSION_HEADER, "1")
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"initialized"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body_text(response).await,
            "Acp-Protocol-Version header does not match initialized protocol version"
        );

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }
}
