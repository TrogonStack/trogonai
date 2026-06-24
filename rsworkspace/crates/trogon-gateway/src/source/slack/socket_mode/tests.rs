use super::*;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message as AxumMessage, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{any, post};
use std::error::Error;
use std::future::IntoFuture;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{MaxPayload, MockJetStreamPublisher, MockObjectStore, StreamMaxAge};
use trogon_std::NonZeroDuration;

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        MockObjectStore::new(),
        "test-bucket".to_string(),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn socket_config() -> SlackConfig {
    SlackConfig {
        subject_prefix: NatsToken::new("slack").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        transport: super::super::config::SlackTransportConfig::SocketMode(SlackSocketModeConfig {
            app_token: super::super::config::SlackAppToken::new("xapp-test-token").unwrap(),
        }),
    }
}

#[cfg(not(coverage))]
fn webhook_config() -> SlackConfig {
    SlackConfig {
        subject_prefix: NatsToken::new("slack").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        transport: super::super::config::SlackTransportConfig::Webhook(super::super::config::SlackWebhookConfig {
            signing_secret: super::super::config::SlackSigningSecret::new("slack-secret").unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
        }),
    }
}

fn bridge(publisher: MockJetStreamPublisher) -> SlackBridge<MockJetStreamPublisher, MockObjectStore> {
    SlackBridge::new(wrap_publisher(publisher), &socket_config())
}

#[test]
fn reconnect_delay_doubles_until_cap() {
    assert_eq!(next_reconnect_delay(Duration::from_secs(1)), Duration::from_secs(2));
    assert_eq!(next_reconnect_delay(Duration::from_secs(20)), RECONNECT_MAX_DELAY);
    assert_eq!(next_reconnect_delay(RECONNECT_MAX_DELAY), RECONNECT_MAX_DELAY);
}

#[cfg(not(coverage))]
#[tokio::test]
async fn run_requires_socket_mode_config() {
    let error = run(wrap_publisher(MockJetStreamPublisher::new()), &webhook_config())
        .await
        .unwrap_err();

    assert!(matches!(error, SocketModeError::MissingSocketModeConfig));
    assert_eq!(error.to_string(), "slack socket_mode config is missing");
    assert!(error.source().is_none());
}

#[test]
fn socket_mode_error_sources_are_exposed() {
    let error = SocketModeError::MissingSocketModeConfig;
    assert_eq!(error.to_string(), "slack socket_mode config is missing");
    assert!(error.source().is_none());

    let json_error = serde_json::from_str::<serde_json::Value>("not-json").unwrap_err();
    let error = SocketModeError::from(json_error);
    assert!(error.to_string().contains("JSON parsing failed"));
    assert!(error.source().is_some());

    let error = SocketModeError::WebSocket(tokio_tungstenite::tungstenite::Error::ConnectionClosed);
    assert!(error.to_string().contains("WebSocket failed"));
    assert!(error.source().is_some());

    let error = SocketModeError::Api("invalid_auth".to_string());
    assert_eq!(error.to_string(), "Slack apps.connections.open failed: invalid_auth");
    assert!(error.source().is_none());
}

#[tokio::test]
async fn socket_events_api_payload_publishes_and_acks() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-1",
        "payload": {
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message", "text": "hello" }
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, Some(r#"{"envelope_id":"env-1"}"#.to_string()));
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.event.message");
    assert_eq!(
        messages[0]
            .headers
            .get(super::super::constants::NATS_HEADER_PAYLOAD_KIND)
            .map(|v| v.as_str()),
        Some("event"),
    );
}

#[tokio::test]
async fn socket_interactive_payload_publishes_and_acks() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "interactive",
        "envelope_id": "env-2",
        "payload": {
            "type": "block_actions",
            "trigger_id": "trigger123",
            "team": { "id": "T01ABC" }
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, Some(r#"{"envelope_id":"env-2"}"#.to_string()));
    let messages = publisher.published_messages();
    assert_eq!(messages[0].subject, "slack.interaction.block_actions");
    assert_eq!(
        messages[0]
            .headers
            .get(super::super::constants::NATS_HEADER_PAYLOAD_KIND)
            .map(|v| v.as_str()),
        Some("interaction"),
    );
}

#[tokio::test]
async fn socket_slash_command_payload_publishes_and_acks() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "slash_commands",
        "envelope_id": "env-3",
        "payload": {
            "command": "/trogon",
            "team_id": "T01ABC",
            "trigger_id": "trigger456"
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, Some(r#"{"envelope_id":"env-3"}"#.to_string()));
    let messages = publisher.published_messages();
    assert_eq!(messages[0].subject, "slack.command.trogon");
    assert_eq!(
        messages[0]
            .headers
            .get(super::super::constants::NATS_HEADER_PAYLOAD_KIND)
            .map(|v| v.as_str()),
        Some("command"),
    );
}

#[tokio::test]
async fn failed_publish_does_not_ack() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let envelope = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-4",
        "payload": {
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
}

#[tokio::test]
async fn missing_event_id_publishes_unroutable_and_does_not_ack() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-5",
        "payload": {
            "type": "event_callback",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
    assert_eq!(publisher.published_messages()[0].subject, "slack.unroutable");
}

#[tokio::test]
async fn hello_frame_does_not_ack_or_reconnect() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "hello"
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
}

#[tokio::test]
async fn unknown_frame_publishes_unroutable_and_acks_when_enveloped() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "unsupported",
        "envelope_id": "env-unsupported",
        "payload": { "value": true }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, Some(r#"{"envelope_id":"env-unsupported"}"#.to_string()));
    let messages = publisher.published_messages();
    assert_eq!(messages[0].subject, "slack.unroutable");
    assert_eq!(
        messages[0]
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("unhandled_socket_mode_type"),
    );
}

#[tokio::test]
async fn payload_frame_missing_envelope_id_publishes_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "events_api",
        "payload": {
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "event": { "type": "message" }
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
    assert_eq!(
        publisher.published_messages()[0]
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("missing_envelope_id"),
    );
}

#[tokio::test]
async fn payload_frame_missing_payload_publishes_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-no-payload"
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
    assert_eq!(
        publisher.published_messages()[0]
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("missing_socket_mode_payload"),
    );
}

#[tokio::test]
async fn payload_envelope_with_unexpected_kind_publishes_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let raw_text = r#"{"type":"unexpected","envelope_id":"env-unexpected","payload":{}}"#;
    let envelope = SocketEnvelope {
        kind: "unexpected".to_string(),
        envelope_id: Some("env-unexpected".to_string()),
        payload: Some(serde_json::json!({})),
        reason: None,
    };

    let ack = handle_payload_envelope(&bridge(publisher.clone()), envelope, raw_text)
        .await
        .unwrap();

    assert_eq!(ack, None);
    assert_eq!(
        publisher.published_messages()[0]
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("unhandled_socket_mode_type"),
    );
}

#[tokio::test]
async fn socket_slash_command_missing_command_publishes_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "slash_commands",
        "envelope_id": "env-missing-command",
        "payload": {
            "team_id": "T01ABC",
            "trigger_id": "trigger456"
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
    assert_eq!(
        publisher.published_messages()[0]
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("missing_command"),
    );
}

#[tokio::test]
async fn socket_slash_command_missing_trigger_publishes_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "slash_commands",
        "envelope_id": "env-missing-trigger",
        "payload": {
            "command": "/trogon",
            "team_id": "T01ABC"
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
    assert_eq!(
        publisher.published_messages()[0]
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("missing_command_trigger_id"),
    );
}

#[derive(Clone)]
struct OpenState {
    ws_url: String,
    seen_auth: mpsc::UnboundedSender<String>,
}

#[derive(Clone)]
struct OpenBodyState {
    body: &'static str,
}

#[derive(Clone)]
struct TextWsState {
    text: String,
    seen_ack: mpsc::UnboundedSender<String>,
}

async fn open_handler(State(state): State<OpenState>, headers: axum::http::HeaderMap) -> String {
    let auth = headers
        .get(reqwest::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    state.seen_auth.send(auth).unwrap();
    serde_json::json!({ "ok": true, "url": state.ws_url }).to_string()
}

async fn open_body_handler(State(state): State<OpenBodyState>) -> String {
    state.body.to_string()
}

async fn open_status_handler() -> (StatusCode, &'static str) {
    (StatusCode::INTERNAL_SERVER_ERROR, "failed")
}

async fn ws_handler(ws: WebSocketUpgrade, State(sender): State<mpsc::UnboundedSender<()>>) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        sender.send(()).unwrap();
        socket.close().await.unwrap();
    })
}

async fn text_ws_handler(ws: WebSocketUpgrade, State(state): State<TextWsState>) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        socket.send(AxumMessage::Text(state.text.into())).await.unwrap();
        if let Some(Ok(AxumMessage::Text(ack))) = socket.recv().await {
            state.seen_ack.send(ack.to_string()).unwrap();
        }
        socket.close().await.unwrap();
    })
}

async fn disconnect_ws_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        socket
            .send(AxumMessage::Text(r#"{"type":"disconnect"}"#.into()))
            .await
            .unwrap();
    })
}

async fn control_ws_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        socket
            .send(AxumMessage::Ping(Bytes::from_static(b"ping")))
            .await
            .unwrap();
        socket
            .send(AxumMessage::Binary(Bytes::from_static(b"payload")))
            .await
            .unwrap();
        socket.close().await.unwrap();
    })
}

async fn spawn_server(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, app).into_future());
    addr
}

#[tokio::test]
async fn apps_connections_open_response_url_is_used() {
    let (ws_seen_tx, mut ws_seen_rx) = mpsc::unbounded_channel();
    let ws_addr = spawn_server(Router::new().route("/socket", any(ws_handler)).with_state(ws_seen_tx)).await;
    let (auth_tx, mut auth_rx) = mpsc::unbounded_channel();
    let open_addr = spawn_server(
        Router::new()
            .route("/apps.connections.open", post(open_handler))
            .with_state(OpenState {
                ws_url: format!("ws://{ws_addr}/socket"),
                seen_auth: auth_tx,
            }),
    )
    .await;

    let config = socket_config();
    let client = reqwest::Client::new();
    let url = open_socket_url(
        &client,
        &format!("http://{open_addr}/apps.connections.open"),
        config.socket_mode().unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(url, format!("ws://{ws_addr}/socket"));
    assert_eq!(auth_rx.recv().await.as_deref(), Some("Bearer xapp-test-token"));

    let bridge = bridge(MockJetStreamPublisher::new());
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        connect_once(
            &client,
            &format!("http://{open_addr}/apps.connections.open"),
            &bridge,
            config.socket_mode().unwrap(),
        ),
    )
    .await;

    result.unwrap().unwrap();
    assert!(ws_seen_rx.recv().await.is_some());
}

#[tokio::test]
async fn apps_connections_open_missing_url_is_api_error() {
    let open_addr = spawn_server(
        Router::new()
            .route("/apps.connections.open", post(open_body_handler))
            .with_state(OpenBodyState { body: r#"{"ok":true}"# }),
    )
    .await;

    let config = socket_config();
    let error = open_socket_url(
        &reqwest::Client::new(),
        &format!("http://{open_addr}/apps.connections.open"),
        config.socket_mode().unwrap(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, SocketModeError::Api(ref message) if message == "missing url"));
}

#[tokio::test]
async fn apps_connections_open_error_is_api_error() {
    let open_addr = spawn_server(
        Router::new()
            .route("/apps.connections.open", post(open_body_handler))
            .with_state(OpenBodyState {
                body: r#"{"ok":false,"error":"invalid_auth"}"#,
            }),
    )
    .await;

    let config = socket_config();
    let error = open_socket_url(
        &reqwest::Client::new(),
        &format!("http://{open_addr}/apps.connections.open"),
        config.socket_mode().unwrap(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, SocketModeError::Api(ref message) if message == "invalid_auth"));
}

#[tokio::test]
async fn apps_connections_open_http_error_exposes_source() {
    let open_addr = spawn_server(Router::new().route("/apps.connections.open", post(open_status_handler))).await;

    let config = socket_config();
    let error = open_socket_url(
        &reqwest::Client::new(),
        &format!("http://{open_addr}/apps.connections.open"),
        config.socket_mode().unwrap(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, SocketModeError::Http(_)));
    assert!(error.to_string().contains("HTTP request failed"));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn connect_once_websocket_error_exposes_source() {
    let open_addr = spawn_server(
        Router::new()
            .route("/apps.connections.open", post(open_body_handler))
            .with_state(OpenBodyState {
                body: r#"{"ok":true,"url":"ws://127.0.0.1:9/socket"}"#,
            }),
    )
    .await;

    let config = socket_config();
    let bridge = bridge(MockJetStreamPublisher::new());
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        connect_once(
            &reqwest::Client::new(),
            &format!("http://{open_addr}/apps.connections.open"),
            &bridge,
            config.socket_mode().unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();

    assert!(matches!(error, SocketModeError::WebSocket(_)));
    assert!(error.to_string().contains("WebSocket failed"));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn connect_once_sends_ack_for_successful_text_frame() {
    let envelope = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-connect",
        "payload": {
            "type": "event_callback",
            "event_id": "EvConnect",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }
    });
    let (ack_tx, mut ack_rx) = mpsc::unbounded_channel();
    let ws_addr = spawn_server(
        Router::new()
            .route("/socket", any(text_ws_handler))
            .with_state(TextWsState {
                text: envelope.to_string(),
                seen_ack: ack_tx,
            }),
    )
    .await;
    let open_addr = spawn_server(
        Router::new()
            .route("/apps.connections.open", post(open_body_handler))
            .with_state(OpenBodyState {
                body: Box::leak(
                    serde_json::json!({ "ok": true, "url": format!("ws://{ws_addr}/socket") })
                        .to_string()
                        .into_boxed_str(),
                ),
            }),
    )
    .await;

    let config = socket_config();
    let bridge = bridge(MockJetStreamPublisher::new());
    connect_once(
        &reqwest::Client::new(),
        &format!("http://{open_addr}/apps.connections.open"),
        &bridge,
        config.socket_mode().unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(ack_rx.recv().await.as_deref(), Some(r#"{"envelope_id":"env-connect"}"#),);
}

#[tokio::test]
async fn connect_once_reconnects_on_disconnect_frame() {
    let ws_addr = spawn_server(Router::new().route("/socket", any(disconnect_ws_handler))).await;
    let open_addr = spawn_server(
        Router::new()
            .route("/apps.connections.open", post(open_body_handler))
            .with_state(OpenBodyState {
                body: Box::leak(
                    serde_json::json!({ "ok": true, "url": format!("ws://{ws_addr}/socket") })
                        .to_string()
                        .into_boxed_str(),
                ),
            }),
    )
    .await;

    let config = socket_config();
    let bridge = bridge(MockJetStreamPublisher::new());
    connect_once(
        &reqwest::Client::new(),
        &format!("http://{open_addr}/apps.connections.open"),
        &bridge,
        config.socket_mode().unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn connect_once_handles_control_frames() {
    let ws_addr = spawn_server(Router::new().route("/socket", any(control_ws_handler))).await;
    let open_addr = spawn_server(
        Router::new()
            .route("/apps.connections.open", post(open_body_handler))
            .with_state(OpenBodyState {
                body: Box::leak(
                    serde_json::json!({ "ok": true, "url": format!("ws://{ws_addr}/socket") })
                        .to_string()
                        .into_boxed_str(),
                ),
            }),
    )
    .await;

    let config = socket_config();
    let bridge = bridge(MockJetStreamPublisher::new());
    connect_once(
        &reqwest::Client::new(),
        &format!("http://{open_addr}/apps.connections.open"),
        &bridge,
        config.socket_mode().unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn process_socket_messages_returns_when_stream_ends() {
    let mut receiver = futures_util::stream::empty::<Result<Message, WebSocketError>>();
    let mut sender = futures_util::sink::drain::<Message>()
        .sink_map_err(|never: std::convert::Infallible| -> WebSocketError { match never {} });

    process_socket_messages(&bridge(MockJetStreamPublisher::new()), &mut receiver, &mut sender)
        .await
        .unwrap();
}

#[tokio::test]
async fn disconnect_frame_completes_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "disconnect",
        "reason": "refresh_requested"
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher), &envelope.to_string())
        .await
        .unwrap();

    assert!(reconnect);
    assert_eq!(ack, None);
}

#[tokio::test]
async fn disconnect_frame_defaults_missing_reason_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "disconnect"
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher), &envelope.to_string())
        .await
        .unwrap();

    assert!(reconnect);
    assert_eq!(ack, None);
}
