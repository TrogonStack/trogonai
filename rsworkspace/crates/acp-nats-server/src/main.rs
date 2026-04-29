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
    let server_config = config::config_from_args(args, &SystemEnv)?;
    acp_telemetry::init_logger(
        ServiceName::AcpNatsServer,
        server_config.acp.acp_prefix(),
        &SystemEnv,
        &SystemFs,
    );
    let server_config = config::apply_timeout_overrides(server_config, &SystemEnv);

    info!("ACP remote transport bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(server_config.acp.nats(), nats_connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(js_context);

    let (shutdown_tx, _) = watch::channel(false);
    let (manager_tx, manager_rx) = mpsc::unbounded_channel::<ManagerRequest>();

    let conn_thread = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || run_connection_thread(manager_rx, nats_client, js_client, server_config.acp))?;

    let state = AppState {
        bind_host: server_config.host,
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

    let addr = SocketAddr::from((server_config.host, server_config.port));
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
    rt.block_on(local.run_until(process_connections(manager_rx, nats_client, js_client, config)));

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

    let active = websocket_handles.iter().filter(|h| !h.is_finished()).count()
        + http_connection_handles.iter().filter(|h| !h.is_finished()).count();
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
    use crate::constants::{ACP_CONNECTION_ID_HEADER, ACP_PROTOCOL_VERSION_HEADER, ACP_SESSION_ID_HEADER};
    use acp_nats::Config;
    use agent_client_protocol::{ContentBlock, ContentChunk, SessionNotification, SessionUpdate};
    use axum::body::{Body, to_bytes};
    use axum::http::header::{ACCEPT, CONTENT_TYPE};
    use axum::http::{Request, StatusCode};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
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
        type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: async_nats::HeaderMap,
            payload: bytes::Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            self.publisher.publish_with_headers(subject, headers, payload).await
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
    ) -> (axum::Router, watch::Sender<bool>, std::thread::JoinHandle<()>) {
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

    async fn next_json_sse_event<S>(stream: &mut S) -> Value
    where
        S: futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    {
        let mut buffer = String::new();

        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("timeout waiting for SSE chunk")
                .expect("SSE stream ended")
                .expect("failed to read SSE chunk");
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            let mut consumed = 0usize;
            while let Some(relative_end) = buffer[consumed..].find('\n') {
                let end = consumed + relative_end;
                let line = buffer[consumed..end].trim_end_matches('\r');
                consumed = end + 1;

                if let Some(json) = line.strip_prefix("data: ")
                    && !json.is_empty()
                {
                    return serde_json::from_str(json).unwrap();
                }
            }

            if consumed > 0 {
                buffer.drain(..consumed);
            }
        }
    }

    #[tokio::test]
    async fn next_json_sse_event_skips_empty_event_frames() {
        let mut stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(bytes::Bytes::from("data: \n\n")),
            Ok::<_, reqwest::Error>(bytes::Bytes::from(
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"session/update\"}\n\n",
            )),
        ]);

        let event = next_json_sse_event(&mut stream).await;

        assert_eq!(
            event,
            json!({
                "jsonrpc": "2.0",
                "method": "session/update",
            })
        );
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
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": 0}}"#;
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
                let expected: serde_json::Value = serde_json::from_str(expected_ws_response).unwrap();
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

        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": 0}}"#;
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
    async fn streamable_http_initialize_returns_connection_id_and_json_response() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
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
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "application/json");

        let connection_id = response
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(crate::acp_connection_id::AcpConnectionId::parse(&connection_id).is_ok());

        let body: Value = serde_json::from_str(&body_text(response).await).unwrap();
        assert_eq!(body["id"], 1);
        assert_eq!(body["result"]["protocolVersion"], 0);
        assert_eq!(body["result"]["connectionId"], connection_id);

        shutdown_tx.send(true).unwrap();
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_session_new_returns_accepted_and_get_stream_event() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
                .into(),
        );
        nats_mock.set_response("acp.agent.session.new", r#"{"sessionId":"test-session-1"}"#.into());

        let (addr, shutdown_tx, server_task, conn_thread) = start_test_server(nats_mock).await;
        let client = reqwest::Client::builder().build().unwrap();
        let url = format!("http://{}{}", addr, ACP_ENDPOINT);

        let initialize = client
            .post(&url)
            .header(CONTENT_TYPE.as_str(), "application/json")
            .header(ACCEPT.as_str(), "application/json, text/event-stream")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(initialize.status(), StatusCode::OK);
        let connection_id = initialize
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let _body: Value = serde_json::from_str(&initialize.text().await.unwrap()).unwrap();

        let get = client
            .get(&url)
            .header(ACCEPT.as_str(), "text/event-stream")
            .header(ACP_CONNECTION_ID_HEADER, &connection_id)
            .header(ACP_PROTOCOL_VERSION_HEADER, "0")
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let mut stream = get.bytes_stream();

        let session_new = client
            .post(&url)
            .header(CONTENT_TYPE.as_str(), "application/json")
            .header(ACCEPT.as_str(), "application/json, text/event-stream")
            .header(ACP_CONNECTION_ID_HEADER, &connection_id)
            .header(ACP_PROTOCOL_VERSION_HEADER, "0")
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":".","mcpServers":[]}}"#)
            .send()
            .await
            .unwrap();

        assert_eq!(session_new.status(), StatusCode::ACCEPTED);
        assert!(session_new.text().await.unwrap().is_empty());

        let event = next_json_sse_event(&mut stream).await;
        assert_eq!(event["id"], 2);
        assert_eq!(event["result"]["sessionId"], "test-session-1");

        shutdown_tx.send(true).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task did not shut down");
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_session_load_uses_request_session_id_header() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
                .into(),
        );
        nats_mock.set_response("acp.session.test-session-1.agent.load", "{}".into());

        let (addr, shutdown_tx, server_task, conn_thread) = start_test_server(nats_mock).await;
        let client = reqwest::Client::builder().build().unwrap();
        let url = format!("http://{}{}", addr, ACP_ENDPOINT);

        let initialize = client
            .post(&url)
            .header(CONTENT_TYPE.as_str(), "application/json")
            .header(ACCEPT.as_str(), "application/json, text/event-stream")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(initialize.status(), StatusCode::OK);
        let connection_id = initialize
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let _body: Value = serde_json::from_str(&initialize.text().await.unwrap()).unwrap();

        let get = client
            .get(&url)
            .header(ACCEPT.as_str(), "text/event-stream")
            .header(ACP_CONNECTION_ID_HEADER, &connection_id)
            .header(ACP_PROTOCOL_VERSION_HEADER, "0")
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let mut stream = get.bytes_stream();

        let session_load = client
            .post(&url)
            .header(CONTENT_TYPE.as_str(), "application/json")
            .header(ACCEPT.as_str(), "application/json, text/event-stream")
            .header(ACP_CONNECTION_ID_HEADER, &connection_id)
            .header(ACP_PROTOCOL_VERSION_HEADER, "0")
            .header(ACP_SESSION_ID_HEADER, "test-session-1")
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"test-session-1","cwd":".","mcpServers":[]}}"#)
            .send()
            .await
            .unwrap();

        assert_eq!(session_load.status(), StatusCode::ACCEPTED);
        assert!(session_load.text().await.unwrap().is_empty());

        let event = next_json_sse_event(&mut stream).await;
        assert_eq!(event["id"], 2);

        shutdown_tx.send(true).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task did not shut down");
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_session_load_requires_session_header() {
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
                    .header(ACP_PROTOCOL_VERSION_HEADER, "0")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"test-session-1","cwd":".","mcpServers":[]}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_text(response).await, "missing Acp-Session-Id header");

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_session_load_returns_accepted_before_backend_completes() {
        let nats_mock = AdvancedMockNatsClient::new();
        let control = nats_mock.clone();
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

        control.hang_next_request();

        let session_load = tokio::time::timeout(
            Duration::from_millis(200),
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri(ACP_ENDPOINT)
                    .header(CONTENT_TYPE, "application/json")
                    .header(ACCEPT, "application/json, text/event-stream")
                    .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                    .header(ACP_PROTOCOL_VERSION_HEADER, "0")
                    .header(ACP_SESSION_ID_HEADER, "test-session-1")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"test-session-1","cwd":".","mcpServers":[]}}"#,
                    ))
                    .unwrap(),
            ),
        )
        .await
        .expect("session/load should return 202 before the backend completes")
        .unwrap();

        assert_eq!(session_load.status(), StatusCode::ACCEPTED);

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
            .oneshot(Request::builder().method("GET").uri("/ws").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_rejects_follow_up_requests_before_successful_initialize() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.fail_next_request();

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
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"initialized"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_text(response).await, "ACP connection has not been initialized");

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_get_rejects_before_successful_initialize() {
        let nats_mock = AdvancedMockNatsClient::new();
        let _injector = nats_mock.inject_messages();
        nats_mock.fail_next_request();

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
                    .method("GET")
                    .uri(ACP_ENDPOINT)
                    .header(ACCEPT, "text/event-stream")
                    .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                    .header(ACP_SESSION_ID_HEADER, "test-session-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body_text(response).await, "ACP connection has not been initialized");

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
    async fn streamable_http_rejects_unknown_session_scoped_notification() {
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
                    .header(ACP_PROTOCOL_VERSION_HEADER, "0")
                    .header(ACP_SESSION_ID_HEADER, "ghost-session")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"ghost-session"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_text(response).await, "unknown ACP session");

        let _ = shutdown_tx.send(true);
        drop(app);
        conn_thread.join().unwrap();
    }

    #[tokio::test]
    async fn streamable_http_rejects_unknown_session_scoped_post() {
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
                    .header(ACP_PROTOCOL_VERSION_HEADER, "0")
                    .header(ACP_SESSION_ID_HEADER, "ghost-session")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"ghost-session","prompt":{"role":"user","content":[{"type":"text","text":"hi"}]}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_text(response).await, "unknown ACP session");

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

    #[tokio::test]
    async fn streamable_http_get_broadcasts_connection_stream_updates_to_all_active_listeners() {
        let nats_mock = AdvancedMockNatsClient::new();
        let notification_tx = nats_mock.inject_messages();
        nats_mock.set_response(
            "acp.agent.initialize",
            r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#
                .into(),
        );
        nats_mock.set_response("acp.agent.session.new", r#"{"sessionId":"test-session-1"}"#.into());

        let (addr, shutdown_tx, server_task, conn_thread) = start_test_server(nats_mock).await;
        let client = reqwest::Client::builder().build().unwrap();
        let url = format!("http://{}{}", addr, ACP_ENDPOINT);

        let initialize = client
            .post(&url)
            .header(CONTENT_TYPE.as_str(), "application/json")
            .header(ACCEPT.as_str(), "application/json, text/event-stream")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(initialize.status(), StatusCode::OK);
        let connection_id = initialize
            .headers()
            .get(ACP_CONNECTION_ID_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let _body: Value = serde_json::from_str(&initialize.text().await.unwrap()).unwrap();

        let first_get = client
            .get(&url)
            .header(ACCEPT.as_str(), "text/event-stream")
            .header(ACP_CONNECTION_ID_HEADER, &connection_id)
            .header(ACP_PROTOCOL_VERSION_HEADER, "0")
            .send()
            .await
            .unwrap();
        let second_get = client
            .get(&url)
            .header(ACCEPT.as_str(), "text/event-stream")
            .header(ACP_CONNECTION_ID_HEADER, &connection_id)
            .header(ACP_PROTOCOL_VERSION_HEADER, "0")
            .send()
            .await
            .unwrap();

        assert_eq!(first_get.status(), StatusCode::OK);
        assert_eq!(second_get.status(), StatusCode::OK);

        let mut first_stream = first_get.bytes_stream();
        let mut second_stream = second_get.bytes_stream();

        let session_new = client
            .post(&url)
            .header(CONTENT_TYPE.as_str(), "application/json")
            .header(ACCEPT.as_str(), "application/json, text/event-stream")
            .header(ACP_CONNECTION_ID_HEADER, &connection_id)
            .header(ACP_PROTOCOL_VERSION_HEADER, "0")
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":".","mcpServers":[]}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(session_new.status(), StatusCode::ACCEPTED);
        assert!(session_new.text().await.unwrap().is_empty());

        let first_session_event = next_json_sse_event(&mut first_stream).await;
        let second_session_event = next_json_sse_event(&mut second_stream).await;
        assert_eq!(first_session_event["id"], 2);
        assert_eq!(second_session_event["id"], 2);

        let session_id = "test-session-1".to_string();
        assert_eq!(first_session_event["result"]["sessionId"], session_id);
        assert_eq!(second_session_event["result"]["sessionId"], session_id);

        let notification = SessionNotification::new(
            session_id.clone(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("fanout"))),
        );
        let payload = serde_json::to_vec(&notification).unwrap();
        notification_tx
            .unbounded_send(async_nats::Message {
                subject: format!("acp.session.{}.client.session.update", session_id).into(),
                reply: None,
                payload: payload.clone().into(),
                headers: None,
                length: payload.len(),
                status: None,
                description: None,
            })
            .unwrap();

        let expected = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": serde_json::to_value(notification).unwrap(),
        });
        let (first_event, second_event) = tokio::join!(
            next_json_sse_event(&mut first_stream),
            next_json_sse_event(&mut second_stream)
        );

        assert_eq!(first_event, expected);
        assert_eq!(second_event, expected);

        drop(first_stream);
        drop(second_stream);
        shutdown_tx.send(true).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server task did not shut down");
        conn_thread.join().unwrap();
    }

    // ── E2E tests: real NATS + TrogonAgent ────────────────────────────────────
    mod http_runner_e2e {
        use crate::constants::{ACP_CONNECTION_ID_HEADER, ACP_ENDPOINT, ACP_PROTOCOL_VERSION_HEADER, THREAD_NAME};
        use crate::transport::{AppState, ManagerRequest};
        use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
        use acp_nats_agent::AgentSideNatsConnection;
        use async_nats::jetstream;
        use serde_json::Value;
        use std::net::{IpAddr, Ipv4Addr};
        use std::sync::Arc;
        use std::time::Duration;
        use testcontainers_modules::nats::Nats;
        use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
        use tokio::net::TcpListener;
        use tokio::sync::{RwLock, mpsc, watch};
        use trogon_acp_runner::{NatsSessionNotifier, NatsSessionStore, TrogonAgent};
        use trogon_agent_core::{agent_loop::AgentLoop, tools::ToolContext};
        use trogon_nats::jetstream::NatsJetStreamClient;

        async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, u16) {
            let container = Nats::default()
                .with_cmd(["--jetstream"])
                .start()
                .await
                .expect("Failed to start NATS container — is Docker running?");
            let port = container.get_host_port_ipv4(4222).await.unwrap();
            let nats = async_nats::connect(format!("127.0.0.1:{port}"))
                .await
                .expect("connect to NATS");
            (container, nats, port)
        }

        fn make_agent_loop() -> AgentLoop {
            let http = reqwest::Client::new();
            AgentLoop {
                http_client: http.clone(),
                proxy_url: String::new(),
                anthropic_token: String::new(),
                anthropic_base_url: None,
                anthropic_extra_headers: vec![],
                model: "claude-opus-4-6".to_string(),
                max_iterations: 10,
                thinking_budget: None,
                tool_context: Arc::new(ToolContext { http_client: http, proxy_url: String::new() }),
                memory_owner: None,
                memory_repo: None,
                memory_path: None,
                mcp_tool_defs: vec![],
                mcp_dispatch: vec![],
                permission_checker: None,
            }
        }

        async fn start_agent(nats: async_nats::Client) {
            let gateway_config = Arc::new(RwLock::new(None));
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local = tokio::task::LocalSet::new();
                let store = rt.block_on(async {
                    let js = jetstream::new(nats.clone());
                    NatsSessionStore::open(&js).await.unwrap()
                });
                let ta = TrogonAgent::new(
                    NatsSessionNotifier::new(nats.clone()),
                    store,
                    make_agent_loop(),
                    "acp",
                    "claude-opus-4-6",
                    None,
                    gateway_config,
                );
                let prefix = AcpPrefix::new("acp").unwrap();
                let (_, io_task) = AgentSideNatsConnection::new(ta, nats, prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
                rt.block_on(local.run_until(async move { io_task.await.ok(); }));
            });
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        async fn start_server(
            nats_port: u16,
        ) -> (String, watch::Sender<bool>, std::thread::JoinHandle<()>) {
            let nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
                .await
                .expect("connect for HTTP bridge");
            let js = NatsJetStreamClient::new(jetstream::new(nats.clone()));
            let config = Config::new(
                AcpPrefix::new("acp").unwrap(),
                NatsConfig {
                    servers: vec![format!("127.0.0.1:{nats_port}")],
                    auth: NatsAuth::None,
                },
            )
            .with_operation_timeout(Duration::from_secs(5));

            let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
            let (manager_tx, manager_rx) = mpsc::unbounded_channel::<ManagerRequest>();

            let conn_thread = std::thread::Builder::new()
                .name(THREAD_NAME.into())
                .spawn(move || crate::run_connection_thread(manager_rx, nats, js, config))
                .expect("spawn connection thread");

            let state = AppState {
                bind_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                manager_tx,
                shutdown_tx: shutdown_tx.clone(),
            };

            let router = axum::Router::new()
                .route(
                    ACP_ENDPOINT,
                    axum::routing::get(crate::transport::get)
                        .post(crate::transport::post)
                        .delete(crate::transport::delete),
                )
                .with_state(state);

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.changed().await;
                    })
                    .await
                    .unwrap();
            });

            (format!("http://{addr}"), shutdown_tx, conn_thread)
        }

        /// E2E: HTTP bridge → NATS → TrogonAgent → back.
        /// TrogonAgent handles `initialize` and returns real capabilities.
        #[tokio::test]
        async fn e2e_http_initialize_with_real_agent() {
            let (_container, nats, nats_port) = start_nats().await;
            start_agent(nats).await;
            let (base_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

            let client = reqwest::Client::new();
            let url = format!("{base_url}{ACP_ENDPOINT}");

            let response = tokio::time::timeout(
                Duration::from_secs(10),
                client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#)
                    .send(),
            )
            .await
            .expect("timed out waiting for initialize response")
            .unwrap();

            assert_eq!(response.status(), 200u16);
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["id"], 1);
            assert!(
                body["result"]["protocolVersion"].is_number(),
                "must have protocolVersion: {body}"
            );

            shutdown_tx.send(true).unwrap();
            let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
        }

        /// E2E: HTTP bridge → NATS → TrogonAgent creates session → SSE delivers session ID.
        #[tokio::test]
        async fn e2e_http_session_new_with_real_agent() {
            let (_container, nats, nats_port) = start_nats().await;
            start_agent(nats).await;
            let (base_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

            let client = reqwest::Client::builder().build().unwrap();
            let url = format!("{base_url}{ACP_ENDPOINT}");

            // Initialize to obtain connection_id.
            let init_resp = tokio::time::timeout(
                Duration::from_secs(10),
                client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#)
                    .send(),
            )
            .await
            .expect("timed out on initialize")
            .unwrap();
            assert_eq!(init_resp.status(), 200u16);
            let connection_id = init_resp
                .headers()
                .get(ACP_CONNECTION_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let init_body: Value = init_resp.json().await.unwrap();
            // Use the negotiated protocol version for all subsequent requests.
            let proto_ver = init_body["result"]["protocolVersion"]
                .as_u64()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "0".to_string());

            // Open SSE stream.
            let get = client
                .get(&url)
                .header("Accept", "text/event-stream")
                .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                .header(ACP_PROTOCOL_VERSION_HEADER, &proto_ver)
                .send()
                .await
                .unwrap();
            let get_status = get.status();
            if get_status.as_u16() != 200 {
                let body = get.text().await.unwrap_or_default();
                panic!("GET returned {} with body: {:?}", get_status, body);
            }
            let mut stream = get.bytes_stream();

            // session/new — reply arrives asynchronously on the SSE stream.
            let new_resp = client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                .header(ACP_PROTOCOL_VERSION_HEADER, &proto_ver)
                .body(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#)
                .send()
                .await
                .unwrap();
            assert_eq!(new_resp.status(), 202u16);

            let event = tokio::time::timeout(
                Duration::from_secs(10),
                super::next_json_sse_event(&mut stream),
            )
            .await
            .expect("timed out waiting for session/new SSE event");
            assert_eq!(event["id"], 2);
            let session_id = event["result"]["sessionId"].as_str().unwrap_or("");
            assert!(!session_id.is_empty(), "must have non-empty sessionId: {event}");

            shutdown_tx.send(true).unwrap();
            let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
        }
    }

    // ── E2E tests: real NATS + xAI runner (mock HTTP) ─────────────────────────
    mod http_xai_e2e {
        use std::sync::{Arc, OnceLock};
        use std::time::Duration;

        use acp_nats::{AcpPrefix, Config, NatsAuth, NatsConfig};
        use acp_nats_agent::AgentSideNatsConnection;
        use async_nats::jetstream;
        use serde_json::Value;
        use testcontainers_modules::nats::Nats;
        use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
        use tokio::net::TcpListener;
        use tokio::sync::{Mutex, mpsc, watch};
        use trogon_codex_runner::DefaultCodexAgent;
        use trogon_nats::jetstream::NatsJetStreamClient;
        use trogon_xai_runner::{MockXaiHttpClient, NatsSessionNotifier, XaiAgent, XaiEvent};

        use crate::constants::{ACP_CONNECTION_ID_HEADER, ACP_ENDPOINT, ACP_PROTOCOL_VERSION_HEADER, ACP_SESSION_ID_HEADER, THREAD_NAME};
        use crate::transport::{AppState, ManagerRequest};

        static CODEX_BIN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        fn codex_bin_lock() -> &'static Mutex<()> {
            CODEX_BIN_LOCK.get_or_init(Mutex::default)
        }

        /// Locate mock_codex_server binary: test binary lives in target/<profile>/deps/,
        /// the mock binary lives in target/<profile>/.
        fn mock_codex_bin() -> std::path::PathBuf {
            let exe = std::env::current_exe().unwrap();
            let bin = exe.parent().unwrap().parent().unwrap().join("mock_codex_server");
            assert!(
                bin.exists(),
                "mock_codex_server not found at {bin:?} — run: cargo build -p trogon-codex-runner"
            );
            bin
        }

        async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, u16) {
            let container = Nats::default()
                .with_cmd(["--jetstream"])
                .start()
                .await
                .expect("start NATS container — is Docker running?");
            let port = container.get_host_port_ipv4(4222).await.unwrap();
            let nats = async_nats::connect(format!("127.0.0.1:{port}"))
                .await
                .expect("connect to NATS");
            (container, nats, port)
        }

        async fn setup_streams(js: &jetstream::Context) {
            let prefix = AcpPrefix::new("acp").unwrap();
            for config in acp_nats::jetstream::streams::all_configs(&prefix) {
                js.get_or_create_stream(config).await.unwrap();
            }
        }

        async fn start_xai_agent(nats: async_nats::Client, mock: Arc<MockXaiHttpClient>) {
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local = tokio::task::LocalSet::new();
                rt.block_on(local.run_until(async move {
                    let prefix = AcpPrefix::new("acp").unwrap();
                    let js_client = NatsJetStreamClient::new(jetstream::new(nats.clone()));
                    let notifier = NatsSessionNotifier::new(nats.clone(), prefix.clone());
                    let agent = XaiAgent::with_deps(notifier, "grok-4", "dummy", mock);
                    let (_, io_task) = AgentSideNatsConnection::with_jetstream(
                        agent,
                        nats,
                        js_client,
                        prefix,
                        |fut| {
                            tokio::task::spawn_local(fut);
                        },
                    );
                    io_task.await.ok();
                }));
            });
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        async fn start_codex_agent(nats: async_nats::Client) {
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let local = tokio::task::LocalSet::new();
                rt.block_on(local.run_until(async move {
                    let prefix = AcpPrefix::new("acp").unwrap();
                    let js_client = NatsJetStreamClient::new(jetstream::new(nats.clone()));
                    let agent = DefaultCodexAgent::with_nats(nats.clone(), prefix.clone(), "o4-mini");
                    let (_, io_task) = AgentSideNatsConnection::with_jetstream(
                        agent,
                        nats,
                        js_client,
                        prefix,
                        |fut| {
                            tokio::task::spawn_local(fut);
                        },
                    );
                    io_task.await.ok();
                }));
            });
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        async fn start_server(nats_port: u16) -> (String, watch::Sender<bool>, std::thread::JoinHandle<()>) {
            let nats = async_nats::connect(format!("127.0.0.1:{nats_port}"))
                .await
                .expect("connect for HTTP bridge");
            let js = NatsJetStreamClient::new(jetstream::new(nats.clone()));
            let config = Config::new(
                AcpPrefix::new("acp").unwrap(),
                NatsConfig {
                    servers: vec![format!("127.0.0.1:{nats_port}")],
                    auth: NatsAuth::None,
                },
            )
            .with_operation_timeout(Duration::from_secs(10));

            let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
            let (manager_tx, manager_rx) = mpsc::unbounded_channel::<ManagerRequest>();

            let conn_thread = std::thread::Builder::new()
                .name(THREAD_NAME.into())
                .spawn(move || crate::run_connection_thread(manager_rx, nats, js, config))
                .expect("spawn connection thread");

            let state = AppState {
                bind_host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                manager_tx,
                shutdown_tx: shutdown_tx.clone(),
            };

            let router = axum::Router::new()
                .route(
                    ACP_ENDPOINT,
                    axum::routing::get(crate::transport::get)
                        .post(crate::transport::post)
                        .delete(crate::transport::delete),
                )
                .with_state(state);

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.changed().await;
                    })
                    .await
                    .unwrap();
            });

            (format!("http://{addr}"), shutdown_tx, conn_thread)
        }

        /// Drains SSE events until finding one with `"id": expected_id`.
        /// Skips intermediate notifications (session/update, keepalives) that have no id.
        /// Uses a 10-second per-chunk timeout to accommodate JetStream round-trips.
        async fn find_sse_event_with_id<S>(stream: &mut S, expected_id: i64) -> Value
        where
            S: futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
        {
            use futures_util::StreamExt as _;
            let mut buffer = String::new();
            for _ in 0..50usize {
                let chunk = tokio::time::timeout(Duration::from_secs(10), stream.next())
                    .await
                    .expect("timeout waiting for SSE chunk")
                    .expect("SSE stream ended")
                    .expect("failed to read SSE chunk");
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                let mut consumed = 0usize;
                while let Some(relative_end) = buffer[consumed..].find('\n') {
                    let end = consumed + relative_end;
                    let line = buffer[consumed..end].trim_end_matches('\r');
                    consumed = end + 1;
                    if let Some(json) = line.strip_prefix("data: ")
                        && !json.is_empty()
                    {
                        if let Ok(v) = serde_json::from_str::<Value>(json) {
                            if v.get("id").and_then(|x| x.as_i64()) == Some(expected_id) {
                                return v;
                            }
                        }
                    }
                }
                if consumed > 0 {
                    buffer.drain(..consumed);
                }
            }
            panic!("did not receive SSE event with id={expected_id} within 50 chunks");
        }

        async fn run_initialize_and_session_new(
            client: &reqwest::Client,
            url: &str,
        ) -> (String, String, String, impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin) {
            // initialize
            let init_resp = tokio::time::timeout(
                Duration::from_secs(10),
                client
                    .post(url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#)
                    .send(),
            )
            .await
            .expect("timeout on initialize")
            .unwrap();
            assert_eq!(init_resp.status(), 200u16);
            let connection_id = init_resp
                .headers()
                .get(ACP_CONNECTION_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let init_body: Value = init_resp.json().await.unwrap();
            let proto_ver = init_body["result"]["protocolVersion"]
                .as_u64()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "0".to_string());

            // open SSE GET stream
            let get = client
                .get(url)
                .header("Accept", "text/event-stream")
                .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                .header(ACP_PROTOCOL_VERSION_HEADER, &proto_ver)
                .send()
                .await
                .unwrap();
            assert_eq!(get.status(), 200u16);
            let mut stream = get.bytes_stream();

            // session/new
            let session_new_resp = client
                .post(url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                .header(ACP_PROTOCOL_VERSION_HEADER, &proto_ver)
                .body(r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}"#)
                .send()
                .await
                .unwrap();
            assert_eq!(session_new_resp.status(), 202u16);
            drop(session_new_resp);

            let session_event = find_sse_event_with_id(&mut stream, 2).await;
            let session_id = session_event["result"]["sessionId"].as_str().unwrap_or("").to_string();
            assert!(!session_id.is_empty(), "must have non-empty sessionId: {session_event}");

            (connection_id, proto_ver, session_id, stream)
        }

        /// E2E: HTTP client → bridge → NATS → XaiAgent (mock HTTP) → prompt → SSE back.
        /// Verifies the full prompt path without real xAI credentials.
        #[tokio::test]
        async fn e2e_http_prompt_with_xai_runner() {
            let (_container, nats, nats_port) = start_nats().await;
            setup_streams(&jetstream::new(nats.clone())).await;

            let mock = Arc::new(MockXaiHttpClient::default());
            mock.push_response(vec![
                XaiEvent::TextDelta { text: "hello from xai mock".to_string() },
                XaiEvent::Done,
            ]);
            start_xai_agent(nats, mock).await;
            let (base_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

            let client = reqwest::Client::new();
            let url = format!("{base_url}{ACP_ENDPOINT}");

            let (connection_id, proto_ver, session_id, mut stream) =
                run_initialize_and_session_new(&client, &url).await;

            // session/prompt
            let prompt_body = serde_json::json!({
                "jsonrpc": "2.0", "id": 3,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{"type": "text", "text": "ping"}]
                }
            });
            client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                .header(ACP_PROTOCOL_VERSION_HEADER, &proto_ver)
                .header(ACP_SESSION_ID_HEADER, &session_id)
                .body(serde_json::to_string(&prompt_body).unwrap())
                .send()
                .await
                .unwrap();

            let prompt_event = find_sse_event_with_id(&mut stream, 3).await;

            assert!(
                prompt_event["result"]["stopReason"].is_string(),
                "prompt result must have stopReason: {prompt_event}"
            );

            shutdown_tx.send(true).unwrap();
            let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
        }

        /// E2E: HTTP client → bridge → NATS → DefaultCodexAgent (mock binary) → prompt → SSE back.
        /// Verifies the full prompt path without real Codex/OpenAI credentials.
        #[tokio::test]
        async fn e2e_http_prompt_with_codex_runner() {
            let _lock = codex_bin_lock().lock().await;
            // SAFETY: serialized by CODEX_BIN_LOCK within this test binary.
            unsafe { std::env::set_var("CODEX_BIN", mock_codex_bin()) };

            let (_container, nats, nats_port) = start_nats().await;
            setup_streams(&jetstream::new(nats.clone())).await;

            start_codex_agent(nats).await;
            let (base_url, shutdown_tx, conn_thread) = start_server(nats_port).await;

            let client = reqwest::Client::new();
            let url = format!("{base_url}{ACP_ENDPOINT}");

            let (connection_id, proto_ver, session_id, mut stream) =
                run_initialize_and_session_new(&client, &url).await;

            // session/prompt
            let prompt_body = serde_json::json!({
                "jsonrpc": "2.0", "id": 3,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{"type": "text", "text": "ping"}]
                }
            });
            client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header(ACP_CONNECTION_ID_HEADER, &connection_id)
                .header(ACP_PROTOCOL_VERSION_HEADER, &proto_ver)
                .header(ACP_SESSION_ID_HEADER, &session_id)
                .body(serde_json::to_string(&prompt_body).unwrap())
                .send()
                .await
                .unwrap();

            let prompt_event = find_sse_event_with_id(&mut stream, 3).await;

            assert!(
                prompt_event["result"]["stopReason"].is_string(),
                "prompt result must have stopReason: {prompt_event}"
            );

            shutdown_tx.send(true).unwrap();
            let _ = tokio::task::spawn_blocking(move || conn_thread.join()).await;
        }
    }
}
