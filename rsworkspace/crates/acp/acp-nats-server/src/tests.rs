use super::*;
use crate::constants::{ACP_CONNECTION_ID_HEADER, ACP_ENDPOINT, ACP_PROTOCOL_VERSION_HEADER};

/// Upstream owns session-header routing now, so this lives with the tests.
const ACP_SESSION_ID_HEADER: &str = "acp-session-id";
use acp_nats::Config;
use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, SessionNotification, SessionUpdate};
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

/// Wrap an agent result in the canonical JSON-RPC envelope the NATS leg now
/// carries (ADR#0056). The mock replies to every id with the same body, and the
/// ACP client ignores the response id, so a fixed id is enough here.
fn canonical_result(result: &str) -> bytes::Bytes {
    format!(r#"{{"jsonrpc":"2.0","id":1,"result":{result}}}"#).into()
}

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
    type Error = async_nats::jetstream::context::GetStreamError;
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

/// Builds the served router the same way `main` does: the SDK's transport driving
/// `NatsAgentComponent`, wrapped in the compat layers for the behaviors upstream
/// omits. Tests therefore exercise production wiring, not a stand-in.
fn build_test_app(nats_mock: AdvancedMockNatsClient) -> (axum::Router, watch::Sender<bool>) {
    build_upstream_app(nats_mock)
}

async fn start_test_server(
    nats_mock: AdvancedMockNatsClient,
) -> (std::net::SocketAddr, watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (app, shutdown_tx) = build_upstream_app(nats_mock);
    let mut shutdown_rx = shutdown_tx.subscribe();

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

    (addr, shutdown_tx, server_task)
}

/// Opens a session-scoped SSE stream, where upstream delivers any message whose
/// payload carries a `sessionId`.
async fn session_stream(
    client: &reqwest::Client,
    url: &str,
    connection_id: &str,
    session_id: &str,
) -> impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin {
    let response = client
        .get(url)
        .header(ACCEPT.as_str(), "text/event-stream")
        .header(ACP_CONNECTION_ID_HEADER, connection_id)
        .header(ACP_SESSION_ID_HEADER, session_id)
        .header(ACP_PROTOCOL_VERSION_HEADER, "0")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes_stream()
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
    nats_mock.set_response("acp.v1.global.agent.initialize", canonical_result(nats_response));

    let (addr, shutdown_tx, server_task) = start_test_server(nats_mock).await;

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

    let expected_ws_response = r#"{"id":1,"jsonrpc":"2.0","result":{"agentCapabilities":{"auth":{},"loadSession":false,"mcpCapabilities":{"acp":false,"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}}"#;

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
}

#[tokio::test]
async fn test_shutdown_while_connection_active() {
    let nats_mock = AdvancedMockNatsClient::new();

    let _injector = nats_mock.inject_messages();

    nats_mock.hang_next_request();
    let (addr, shutdown_tx, server_task) = start_test_server(nats_mock).await;

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
}

#[tokio::test]
async fn streamable_http_initialize_returns_connection_id_and_json_response() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_test_app(nats_mock);
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
    // Upstream mints the id (UUIDv4) and keeps its generator private, so assert it
    // is present and non-empty rather than validating a format we no longer own.
    assert!(!connection_id.is_empty(), "initialize must return a connection id");

    let body: Value = serde_json::from_str(&body_text(response).await).unwrap();
    assert_eq!(body["id"], 1);
    assert_eq!(body["result"]["protocolVersion"], 0);
    // `connectionId` is not an ACP v1 schema field. The old transport injected it
    // into the result body; upstream returns it only as a header, which is the
    // spec-conformant behavior, so the body must carry no such field.
    assert!(
        body["result"]["connectionId"].is_null(),
        "the connection id belongs in the header, not the payload"
    );

    shutdown_tx.send(true).unwrap();
    drop(app);
}

#[tokio::test]
async fn streamable_http_session_new_returns_accepted_and_get_stream_event() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));
    nats_mock.set_response(
        "acp.v1.global.agent.session.new",
        canonical_result(r#"{"sessionId":"test-session-1"}"#),
    );

    let (addr, shutdown_tx, server_task) = start_test_server(nats_mock).await;
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
}

#[tokio::test]
async fn streamable_http_session_load_uses_request_session_id_header() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));
    nats_mock.set_response("acp.v1.session.test-session-1.agent.load", canonical_result("{}"));

    let (addr, shutdown_tx, server_task) = start_test_server(nats_mock).await;
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

    // Session-scoped: upstream routes an outbound message to the session stream
    // whenever its payload carries `sessionId`, so a `session/load` reply reaches
    // only a GET that named the session.
    let get = client
        .get(&url)
        .header(ACCEPT.as_str(), "text/event-stream")
        .header(ACP_CONNECTION_ID_HEADER, &connection_id)
        .header(ACP_SESSION_ID_HEADER, "test-session-1")
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
}

#[tokio::test]
async fn streamable_http_session_load_falls_back_to_the_params_session_id() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_test_app(nats_mock);
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

    // Upstream recovers the session from `params.sessionId` when the header is
    // absent, so the request is routed rather than rejected. The old transport
    // required the header and answered 400.
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    // Accepted requests carry no body: the reply arrives on the SSE stream.
    assert!(body_text(response).await.is_empty());

    let _ = shutdown_tx.send(true);
    drop(app);
}

#[tokio::test]
async fn streamable_http_session_load_returns_accepted_before_backend_completes() {
    let nats_mock = AdvancedMockNatsClient::new();
    let control = nats_mock.clone();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_test_app(nats_mock);
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
}

#[tokio::test]
async fn streamable_http_get_requires_connection_and_session_headers() {
    let nats_mock = AdvancedMockNatsClient::new();
    let (app, shutdown_tx) = build_test_app(nats_mock);

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
}

#[tokio::test]
async fn legacy_websocket_alias_is_not_routed() {
    let nats_mock = AdvancedMockNatsClient::new();
    let (app, shutdown_tx) = build_test_app(nats_mock);

    let response = app
        .clone()
        .oneshot(Request::builder().method("GET").uri("/ws").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let _ = shutdown_tx.send(true);
    drop(app);
}

#[tokio::test]
async fn streamable_http_rejects_follow_up_requests_before_successful_initialize() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.fail_next_request();

    let (app, shutdown_tx) = build_test_app(nats_mock);
    let initialize = app
        .clone()
        .oneshot(http_post_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#,
        ))
        .await
        .unwrap();
    // A failed initialize leaves no usable connection. The old transport minted the
    // id before dispatching, so a caller could keep using a connection that never
    // initialized; upstream tears it down and returns no id, which is stricter.
    assert!(
        initialize.headers().get(ACP_CONNECTION_ID_HEADER).is_none(),
        "a failed initialize must not hand back a connection id"
    );
    let _ = body_text(initialize).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(ACP_ENDPOINT)
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .body(Body::from(r#"{"jsonrpc":"2.0","method":"initialized"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        response.status().is_client_error(),
        "a request with no initialized connection must be rejected, got {}",
        response.status()
    );

    let _ = shutdown_tx.send(true);
    drop(app);
}

#[tokio::test]
async fn streamable_http_get_rejects_before_successful_initialize() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.fail_next_request();

    let (app, shutdown_tx) = build_test_app(nats_mock);
    let initialize = app
        .clone()
        .oneshot(http_post_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#,
        ))
        .await
        .unwrap();
    // A failed initialize leaves no usable connection. The old transport minted the
    // id before dispatching, so a caller could keep using a connection that never
    // initialized; upstream tears it down and returns no id, which is stricter.
    assert!(
        initialize.headers().get(ACP_CONNECTION_ID_HEADER).is_none(),
        "a failed initialize must not hand back a connection id"
    );
    let _ = body_text(initialize).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(ACP_ENDPOINT)
                .header(ACCEPT, "text/event-stream")
                .header(ACP_SESSION_ID_HEADER, "test-session-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        response.status().is_client_error(),
        "a request with no initialized connection must be rejected, got {}",
        response.status()
    );

    let _ = shutdown_tx.send(true);
    drop(app);
}

#[tokio::test]
async fn streamable_http_delete_terminates_initialized_connection() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_test_app(nats_mock);
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

    let _ = shutdown_tx.send(true);
    drop(app);
}

#[tokio::test]
async fn streamable_http_forwards_an_unknown_session_scoped_notification() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_test_app(nats_mock);
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

    // Same as the POST case: an unknown session is the agent's to reject, so the
    // transport accepts and forwards rather than answering 404 itself.
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    // Accepted requests carry no body: the agent answers on the SSE stream.
    assert!(body_text(response).await.is_empty());

    let _ = shutdown_tx.send(true);
    drop(app);
}

#[tokio::test]
async fn streamable_http_forwards_an_unknown_session_scoped_post() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_test_app(nats_mock);
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

    // Upstream forwards a session-scoped request without checking that the session
    // exists: session lifetime belongs to the agent, not the transport. The old
    // transport tracked known sessions itself and answered 404 here.
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    // Accepted requests carry no body: the agent answers on the SSE stream.
    assert!(body_text(response).await.is_empty());

    let _ = shutdown_tx.send(true);
    drop(app);
}

#[tokio::test]
async fn streamable_http_rejects_mismatched_protocol_version_after_initialize() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_test_app(nats_mock);
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
}

#[tokio::test]
async fn streamable_http_get_broadcasts_connection_stream_updates_to_all_active_listeners() {
    let nats_mock = AdvancedMockNatsClient::new();
    let notification_tx = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));
    nats_mock.set_response(
        "acp.v1.global.agent.session.new",
        canonical_result(r#"{"sessionId":"test-session-1"}"#),
    );

    let (addr, shutdown_tx, server_task) = start_test_server(nats_mock).await;
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

    // The `session/new` reply above fanned out on the connection stream because its
    // request carried no session id yet. A `session/update` always carries one, so
    // upstream routes it to the session stream: still fan-out, but scoped to the
    // session instead of reaching every listener on the connection.
    let mut first_session_stream = session_stream(&client, &url, &connection_id, &session_id).await;
    let mut second_session_stream = session_stream(&client, &url, &connection_id, &session_id).await;

    let notification = SessionNotification::new(
        session_id.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("fanout"))),
    );
    let payload = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": serde_json::to_value(&notification).unwrap(),
    }))
    .unwrap();
    notification_tx
        .unbounded_send(async_nats::Message {
            subject: format!("acp.v1.session.{}.client.session.update", session_id).into(),
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
        next_json_sse_event(&mut first_session_stream),
        next_json_sse_event(&mut second_session_stream)
    );

    assert_eq!(first_event, expected);
    assert_eq!(second_event, expected);

    drop(first_stream);
    drop(second_stream);
    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task did not shut down");
}

/// Builds the router from the SDK's own HTTP transport, driving the NATS bridge
/// through `NatsAgentComponent` instead of the hand-rolled connection actors.
fn build_upstream_app(nats_mock: AdvancedMockNatsClient) -> (axum::Router, watch::Sender<bool>) {
    let (shutdown_tx, _) = watch::channel(false);
    // Production's own assembly, not a parallel one: this is the exact router `main`
    // serves, so a layer or option that only exists on one side cannot hide here.
    let router = build_router(
        nats_mock,
        MockJs::new(),
        test_config(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        &shutdown_tx,
    );
    (router, shutdown_tx)
}

/// The adoption gate: an `initialize` must round-trip from a real HTTP request,
/// through the SDK-owned transport, across the NATS bridge, and back.
#[tokio::test]
async fn upstream_http_transport_round_trips_initialize_through_the_bridge() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    nats_mock.set_response(
            "acp.v1.global.agent.initialize",
            canonical_result(r#"{"agentCapabilities":{"loadSession":true,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#));

    let (app, shutdown_tx) = build_upstream_app(nats_mock);

    let response = app
        .clone()
        .oneshot(http_post_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get(ACP_CONNECTION_ID_HEADER).is_some(),
        "upstream transport must return a connection id"
    );

    let body: Value = serde_json::from_str(&body_text(response).await).unwrap();
    assert_eq!(body["id"], 1, "response must correlate to the request id");
    assert_eq!(
        body["result"]["protocolVersion"], 0,
        "the bridge's NATS reply must reach the HTTP caller"
    );

    shutdown_tx.send(true).unwrap();
}

/// Drives a component connection to completion via the drain signal.
///
/// The round-trip test above leaves the connection open, so the teardown path
/// (drain the client proxy, flush bridge background work, report the close) is
/// never reached there. `into_channel_and_future` is the same entry point
/// `AcpHttpServer` uses per connection, so this exercises production wiring
/// rather than a stand-in.
#[tokio::test]
async fn draining_closes_an_upstream_transport_connection_cleanly() {
    let nats_mock = AdvancedMockNatsClient::new();
    let _injector = nats_mock.inject_messages();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let component = crate::component::NatsAgentComponent::new(
        nats_mock,
        MockJs::new(),
        test_config(),
        trogon_telemetry::meter("acp-nats-server"),
        shutdown_rx,
    );
    let (_channel, connection) =
        agent_client_protocol::ConnectTo::<agent_client_protocol::Client>::into_channel_and_future(component);

    let handle = tokio::spawn(connection);

    // Let the bridge and client proxy come up before asking them to stop, so the
    // drain branch wins the select rather than racing startup.
    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown_tx.send(true).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("draining must close the connection")
        .expect("the connection task must not panic");

    assert!(result.is_ok(), "a drained connection is a clean close: {result:?}");
}
