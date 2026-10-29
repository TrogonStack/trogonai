use super::super::ack_budget::AckBudget;
use super::super::connections_open_error_code::ConnectionsOpenErrorCode;
use super::super::constants::ENVELOPE_CONCURRENCY_LIMIT;
use super::super::constants::{
    CLIENT_PING_INTERVAL, CLIENT_PING_TIMEOUT, DEFAULT_API_BASE_URL, SERVER_PING_TIMEOUT, UNANSWERED_PING_LIMIT,
};
use super::*;
use async_nats::subject::ToSubject;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message as AxumMessage, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{any, post};
use std::error::Error;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding};
use trogon_nats::jetstream::{MaxPayload, MockJetStreamPublisher, MockObjectStore, StreamMaxAge};
use trogon_std::NonZeroDuration;

fn wrap_publisher<P: JetStreamPublisher>(publisher: P) -> ClaimCheckPublisher<P, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn socket_config() -> SlackConfig {
    SlackConfig {
        subject_prefix: NatsToken::new("slack").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        ack_budget: AckBudget::default(),
        transport: super::super::config::SlackTransportConfig::SocketMode(SlackSocketModeConfig {
            app_token: super::super::config::SlackAppToken::new("xapp-test-token").unwrap(),
            api_base_url: super::super::config::SlackApiBaseUrl::new(super::super::constants::DEFAULT_API_BASE_URL)
                .unwrap(),
        }),
    }
}

fn webhook_config() -> SlackConfig {
    SlackConfig {
        subject_prefix: NatsToken::new("slack").unwrap(),
        stream_name: NatsToken::new("SLACK").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        ack_budget: AckBudget::default(),
        transport: super::super::config::SlackTransportConfig::Webhook(super::super::config::SlackWebhookConfig {
            signing_secret: super::super::config::SlackSigningSecret::new("slack-secret").unwrap(),
            timestamp_max_drift: NonZeroDuration::from_secs(300).unwrap(),
            max_body_size: super::super::constants::DEFAULT_HTTP_BODY_SIZE_MAX,
        }),
    }
}

fn bridge<P: JetStreamPublisher>(publisher: P) -> SlackBridge<P, MockObjectStore, AdvancedMockNatsClient> {
    bridge_with_requester(publisher, acknowledging_requester())
}

fn bridge_with_requester<P: JetStreamPublisher>(
    publisher: P,
    requester: AdvancedMockNatsClient,
) -> SlackBridge<P, MockObjectStore, AdvancedMockNatsClient> {
    SlackBridge::new(wrap_publisher(publisher), requester, &socket_config())
}

fn acknowledging_requester() -> AdvancedMockNatsClient {
    let requester = AdvancedMockNatsClient::new();
    for subject in [
        "slack.respond.command.trogon",
        "slack.respond.interaction.view_submission",
    ] {
        requester.set_response(subject, Bytes::new());
    }
    requester
}

fn replying_requester(subject: &str, content_type: Option<&str>, body: &'static [u8]) -> AdvancedMockNatsClient {
    let requester = AdvancedMockNatsClient::new();
    let mut headers = async_nats::HeaderMap::new();
    if let Some(content_type) = content_type {
        headers.insert("Content-Type", content_type);
    }
    requester.set_response_wire(subject, headers, Bytes::from_static(body));
    requester
}

fn slash_envelope(accepts_response_payload: Option<bool>) -> String {
    let mut envelope = serde_json::json!({
        "type": "slash_commands",
        "envelope_id": "env-3",
        "payload": { "command": "/trogon", "team_id": "T01ABC", "trigger_id": "trigger456" }
    });
    if let Some(accepts) = accepts_response_payload {
        envelope["accepts_response_payload"] = accepts.into();
    }
    envelope.to_string()
}

fn block_suggestion_envelope(accepts_response_payload: bool) -> String {
    serde_json::json!({
        "type": "interactive",
        "envelope_id": "env-5",
        "accepts_response_payload": accepts_response_payload,
        "payload": {
            "type": "block_suggestion",
            "team": { "id": "T01ABC" },
            "action_id": "pick",
            "block_id": "blk",
            "value": "tro"
        }
    })
    .to_string()
}

fn view_submission_envelope() -> String {
    serde_json::json!({
        "type": "interactive",
        "envelope_id": "env-4",
        "accepts_response_payload": true,
        "payload": {
            "type": "view_submission",
            "trigger_id": "trigger789",
            "team": { "id": "T01ABC" },
            "view": { "callback_id": "my_modal" }
        }
    })
    .to_string()
}

#[test]
fn reconnect_delay_doubles_until_cap() {
    assert_eq!(next_reconnect_delay(Duration::from_secs(1)), Duration::from_secs(2));
    assert_eq!(next_reconnect_delay(Duration::from_secs(20)), RECONNECT_MAX_DELAY);
    assert_eq!(next_reconnect_delay(RECONNECT_MAX_DELAY), RECONNECT_MAX_DELAY);
}

struct ScriptedConnector {
    results: std::collections::VecDeque<Result<(), SocketModeError>>,
    attempts: mpsc::Sender<tokio::time::Instant>,
}

impl SocketConnector<MockJetStreamPublisher, MockObjectStore, AdvancedMockNatsClient> for ScriptedConnector {
    async fn connect<'a>(
        &'a mut self,
        _bridge: &'a SlackBridge<MockJetStreamPublisher, MockObjectStore, AdvancedMockNatsClient>,
        config: &'a SlackSocketModeConfig,
    ) -> Result<(), SocketModeError> {
        assert_eq!(config.app_token.as_str(), "xapp-test-token");
        self.attempts.send(tokio::time::Instant::now()).await.unwrap();
        match self.results.pop_front() {
            Some(result) => result,
            None => std::future::pending().await,
        }
    }
}

#[tokio::test(start_paused = true)]
async fn reconnect_failures_back_off_and_success_resets_the_delay() {
    let (attempts, mut observed) = mpsc::channel(1);
    let connector = ScriptedConnector {
        results: [
            Err(SocketModeError::WebSocket(WebSocketError::ConnectionClosed)),
            Err(SocketModeError::WebSocket(WebSocketError::ConnectionClosed)),
            Ok(()),
        ]
        .into(),
        attempts,
    };
    let task = tokio::spawn(run(
        wrap_publisher(MockJetStreamPublisher::new()),
        AdvancedMockNatsClient::new(),
        socket_config(),
        connector,
    ));
    let first = observed.recv().await.unwrap();
    let second = observed.recv().await.unwrap();
    let third = observed.recv().await.unwrap();
    let fourth = observed.recv().await.unwrap();
    assert_eq!(second - first, RECONNECT_INITIAL_DELAY);
    assert_eq!(third - second, RECONNECT_INITIAL_DELAY * 2);
    assert_eq!(fourth - third, RECONNECT_INITIAL_DELAY);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(observed.recv().await.is_none());
}

#[tokio::test]
async fn run_requires_socket_mode_config() {
    let error = run(
        wrap_publisher(MockJetStreamPublisher::new()),
        AdvancedMockNatsClient::new(),
        webhook_config(),
        HttpSocketConnector::new(DEFAULT_API_BASE_URL),
    )
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

    let error = SocketModeError::WebSocket(tokio_tungstenite::tungstenite::Error::ConnectionClosed);
    assert_eq!(
        error.to_string(),
        "Slack Socket Mode WebSocket failed: Connection closed normally"
    );
    assert!(error.source().is_some());

    let error = SocketModeError::Api(ConnectionsOpenErrorCode::new("invalid_auth"));
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
    assert_eq!(
        messages[0]
            .headers
            .get(super::super::constants::NATS_HEADER_CONTENT_TYPE)
            .map(|v| v.as_str()),
        Some("application/json"),
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
        payload: Some(RawValue::from_string("{}".to_string()).unwrap()),
        reason: None,
        retry_attempt: None,
        retry_reason: None,
        accepts_response_payload: None,
    };

    let ack = handle_payload_envelope(&bridge(publisher.clone()), envelope, raw_text).await;

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

#[tokio::test]
async fn unknown_frame_is_not_acked_when_the_unroutable_publish_fails() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let envelope = serde_json::json!({
        "type": "unsupported",
        "envelope_id": "env-unsupported",
        "payload": { "value": true }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn socket_unroutable_carries_message_id_team_and_transport_headers() {
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

    handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    let messages = publisher.published_messages();
    let headers = &messages[0].headers;
    assert_eq!(
        headers
            .get(super::super::constants::NATS_HEADER_TEAM_ID)
            .map(|v| v.as_str()),
        Some("T01ABC"),
    );
    assert_eq!(
        headers
            .get(super::super::constants::NATS_HEADER_TRANSPORT)
            .map(|v| v.as_str()),
        Some("socket_mode"),
    );
    assert!(
        headers
            .get("Nats-Msg-Id")
            .is_some_and(|id| id.as_str().starts_with("unroutable-"))
    );
}

#[tokio::test]
async fn socket_routable_event_is_tagged_with_its_transport() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-1",
        "payload": {
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }
    });

    handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert_eq!(
        publisher.published_messages()[0]
            .headers
            .get(super::super::constants::NATS_HEADER_TRANSPORT)
            .map(|v| v.as_str()),
        Some("socket_mode"),
    );
}

fn envelope_text(kind: &str, payload: &str) -> String {
    format!(r#"{{"type":"{kind}","envelope_id":"env-raw","accepts_response_payload":false,"payload":{payload}}}"#)
}

#[tokio::test]
async fn events_api_payload_is_published_byte_for_byte() {
    let publisher = MockJetStreamPublisher::new();
    let payload = "{\"z\":1,  \"event\" :{\"type\":\"message\",\"text\":\"caf\\u00e9 \\/ \\\"quoted\\\"\"},\n\t\"event_id\":\"Ev01ABC123\",\"team_id\":\"T01ABC\",\"a\":2.50,\"type\":\"event_callback\"}";

    let (_, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope_text("events_api", payload))
        .await
        .unwrap();

    assert_eq!(ack, Some(r#"{"envelope_id":"env-raw"}"#.to_string()));
    let messages = publisher.published_messages();
    assert_eq!(messages[0].subject, "slack.event.message");
    assert_eq!(messages[0].payload.as_ref(), payload.as_bytes());
}

#[tokio::test]
async fn interactive_payload_is_published_byte_for_byte() {
    let publisher = MockJetStreamPublisher::new();
    let payload = "{ \"trigger_id\" : \"trigger123\",\"type\":\"block_actions\",\n  \"team\":{\"id\":\"T01ABC\"}, \"z\":[1,  2] }";

    let (_, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope_text("interactive", payload))
        .await
        .unwrap();

    assert!(ack.is_some());
    assert_eq!(publisher.published_messages()[0].payload.as_ref(), payload.as_bytes());
}

#[tokio::test]
async fn slash_command_payload_is_published_byte_for_byte() {
    let publisher = MockJetStreamPublisher::new();
    let payload = "{\"text\":\"a  b\",  \"trigger_id\":\"trigger456\",\"command\":\"/trogon\" ,\"team_id\":\"T01ABC\"}";

    let (_, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope_text("slash_commands", payload))
        .await
        .unwrap();

    assert!(ack.is_some());
    assert_eq!(publisher.published_messages()[0].payload.as_ref(), payload.as_bytes());
}

#[tokio::test]
async fn slash_command_payload_that_is_not_a_json_value_is_kept_as_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let payload = "{\"command\":\"/trogon\",\"trigger_id\":\"trigger456\",\"team_id\":\"T01ABC\",\"n\":1e999}";

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope_text("slash_commands", payload))
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, None);
    let message = &publisher.published_messages()[0];
    assert_eq!(
        message
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("invalid_json"),
    );
    assert_eq!(message.payload.as_ref(), payload.as_bytes());
}

#[tokio::test]
async fn unroutable_payload_keeps_the_original_bytes() {
    let publisher = MockJetStreamPublisher::new();
    let payload = "{ \"type\" : \"event_callback\",  \"team_id\":\"T01ABC\", \"event\":{\"type\":\"message\"} }";

    handle_text_frame(&bridge(publisher.clone()), &envelope_text("events_api", payload))
        .await
        .unwrap();

    let messages = publisher.published_messages();
    assert_eq!(messages[0].subject, "slack.unroutable");
    assert_eq!(messages[0].payload.as_ref(), payload.as_bytes());
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
        HttpSocketConnector::new(format!("http://{open_addr}/apps.connections.open"))
            .connect(&bridge, config.socket_mode().unwrap()),
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

    assert!(matches!(error, SocketModeError::Api(ref code) if *code == ConnectionsOpenErrorCode::new("missing url")));
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

    assert!(matches!(error, SocketModeError::Api(ref code) if *code == ConnectionsOpenErrorCode::new("invalid_auth")));
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

    let SocketModeError::Http(http_err) = &error else {
        panic!("expected Http error, got {error:?}");
    };
    assert_eq!(
        error.to_string(),
        format!("Slack Socket Mode HTTP request failed: {http_err}")
    );
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

    let SocketModeError::WebSocket(ws_err) = &error else {
        panic!("expected WebSocket error, got {error:?}");
    };
    assert_eq!(
        error.to_string(),
        format!("Slack Socket Mode WebSocket failed: {ws_err}")
    );
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

    process_socket_messages(
        &bridge(MockJetStreamPublisher::new()),
        &mut receiver,
        &mut sender,
        SOCKET_LIVENESS,
    )
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

#[tokio::test]
async fn socket_payloads_with_unsafe_subject_tokens_are_unroutable_and_not_acked() {
    let cases = [
        (
            "events_api",
            serde_json::json!({
                "type": "event_callback",
                "event_id": "Ev01ABC123",
                "team_id": "T01ABC",
                "event": { "type": "a.b" }
            }),
        ),
        (
            "interactive",
            serde_json::json!({
                "type": "block*actions",
                "trigger_id": "trigger123",
                "team": { "id": "T01ABC" }
            }),
        ),
        (
            "slash_commands",
            serde_json::json!({
                "command": "/a b",
                "team_id": "T01ABC",
                "trigger_id": "trigger123"
            }),
        ),
    ];

    for (kind, payload) in cases {
        let publisher = MockJetStreamPublisher::new();
        let envelope = serde_json::json!({
            "type": kind,
            "envelope_id": "env-bad-token",
            "payload": payload
        });

        let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
            .await
            .unwrap();

        assert!(!reconnect);
        assert_eq!(ack, None, "{kind}");
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1, "{kind}");
        assert_eq!(messages[0].subject, "slack.unroutable", "{kind}");
        assert_eq!(
            messages[0]
                .headers
                .get(super::super::constants::NATS_HEADER_REJECT_REASON)
                .map(|v| v.as_str()),
            Some("invalid_subject_token"),
            "{kind}"
        );
    }
}

#[tokio::test]
async fn socket_interaction_without_trigger_id_is_published_and_acked() {
    let publisher = MockJetStreamPublisher::new();
    let envelope = serde_json::json!({
        "type": "interactive",
        "envelope_id": "env-view-closed",
        "payload": {
            "type": "view_closed",
            "team": { "id": "T01ABC" },
            "view": { "id": "V01" }
        }
    });

    let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), &envelope.to_string())
        .await
        .unwrap();

    assert!(!reconnect);
    assert_eq!(ack, Some(r#"{"envelope_id":"env-view-closed"}"#.to_string()));
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.interaction.view_closed");
}

fn retry_envelope(kind: &str, payload: serde_json::Value, retry_reason: &str) -> String {
    serde_json::json!({
        "type": kind,
        "envelope_id": "env-retry",
        "payload": payload,
        "retry_attempt": 2,
        "retry_reason": retry_reason,
    })
    .to_string()
}

fn retry_headers(publisher: &MockJetStreamPublisher) -> (Option<String>, Option<String>) {
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    let header = |name: &str| messages[0].headers.get(name).map(|v| v.as_str().to_owned());
    (
        header(super::super::constants::NATS_HEADER_RETRY_NUM),
        header(super::super::constants::NATS_HEADER_RETRY_REASON),
    )
}

#[tokio::test]
async fn socket_event_redelivery_carries_retry_headers() {
    let publisher = MockJetStreamPublisher::new();
    let frame = retry_envelope(
        "events_api",
        serde_json::json!({
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }),
        "timeout",
    );

    let (_, ack) = handle_text_frame(&bridge(publisher.clone()), &frame).await.unwrap();

    assert_eq!(ack, Some(r#"{"envelope_id":"env-retry"}"#.to_string()));
    assert_eq!(
        retry_headers(&publisher),
        (Some("2".to_string()), Some("timeout".to_string()))
    );
}

#[tokio::test]
async fn socket_interaction_redelivery_carries_retry_headers() {
    let publisher = MockJetStreamPublisher::new();
    let frame = retry_envelope(
        "interactive",
        serde_json::json!({ "type": "block_actions", "trigger_id": "trigger123", "team": { "id": "T01ABC" } }),
        "timeout",
    );

    handle_text_frame(&bridge(publisher.clone()), &frame).await.unwrap();

    assert_eq!(
        retry_headers(&publisher),
        (Some("2".to_string()), Some("timeout".to_string()))
    );
}

#[tokio::test]
async fn socket_slash_command_redelivery_carries_retry_headers() {
    let publisher = MockJetStreamPublisher::new();
    let frame = retry_envelope(
        "slash_commands",
        serde_json::json!({ "command": "/deploy", "trigger_id": "trigger123", "team_id": "T01ABC" }),
        "timeout",
    );

    handle_text_frame(&bridge(publisher.clone()), &frame).await.unwrap();

    assert_eq!(
        retry_headers(&publisher),
        (Some("2".to_string()), Some("timeout".to_string()))
    );
}

#[tokio::test]
async fn socket_first_delivery_has_no_retry_headers() {
    let publisher = MockJetStreamPublisher::new();
    let payload = r#"{"type":"event_callback","event_id":"Ev01ABC123","team_id":"T01ABC","event":{"type":"message"}}"#;

    handle_text_frame(&bridge(publisher.clone()), &envelope_text("events_api", payload))
        .await
        .unwrap();

    assert_eq!(retry_headers(&publisher), (None, None));
}

#[tokio::test]
async fn socket_first_delivery_envelope_with_zero_attempt_and_empty_reason_has_no_retry_headers() {
    let publisher = MockJetStreamPublisher::new();
    let frame = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env-first",
        "payload": {
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        },
        "retry_attempt": 0,
        "retry_reason": "",
    })
    .to_string();

    let (_, ack) = handle_text_frame(&bridge(publisher.clone()), &frame).await.unwrap();

    assert_eq!(ack, Some(r#"{"envelope_id":"env-first"}"#.to_string()));
    assert_eq!(retry_headers(&publisher), (None, None));
}

#[tokio::test]
async fn socket_retry_reason_with_line_break_is_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let frame = retry_envelope(
        "events_api",
        serde_json::json!({
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }),
        "a\r\nX-Injected: 1",
    );

    let (_, ack) = handle_text_frame(&bridge(publisher.clone()), &frame).await.unwrap();

    assert_eq!(ack, None);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.unroutable");
    assert_eq!(
        messages[0]
            .headers
            .get(super::super::constants::NATS_HEADER_REJECT_REASON)
            .map(|v| v.as_str()),
        Some("invalid_header_value"),
    );
}

#[tokio::test]
async fn socket_payloads_carry_enterprise_and_app_identity() {
    let cases = [
        (
            "events_api",
            serde_json::json!({
                "type": "event_callback", "event_id": "Ev01ABC123", "team_id": "T01ABC", "api_app_id": "A01APP",
                "authorizations": [{ "enterprise_id": "E01ENT", "is_enterprise_install": true }],
                "event": { "type": "message" }
            }),
            "true",
        ),
        (
            "interactive",
            serde_json::json!({
                "type": "block_actions", "trigger_id": "trigger123", "api_app_id": "A01APP",
                "enterprise": { "id": "E01ENT" }, "team": { "id": "T01ABC" }, "is_enterprise_install": false
            }),
            "false",
        ),
        (
            "slash_commands",
            serde_json::json!({
                "command": "/deploy", "trigger_id": "trigger123", "team_id": "T01ABC", "api_app_id": "A01APP",
                "enterprise_id": "E01ENT", "is_enterprise_install": "true"
            }),
            "true",
        ),
    ];

    for (kind, payload, install) in cases {
        let publisher = MockJetStreamPublisher::new();
        let frame = serde_json::json!({ "type": kind, "envelope_id": "env-1", "payload": payload }).to_string();

        handle_text_frame(&bridge(publisher.clone()), &frame).await.unwrap();

        let messages = publisher.published_messages();
        let header = |name: &str| messages[0].headers.get(name).map(|v| v.as_str());
        assert_eq!(header("X-Slack-Enterprise-Id"), Some("E01ENT"), "{kind}");
        assert_eq!(header("X-Slack-Is-Enterprise-Install"), Some(install), "{kind}");
        assert_eq!(header("X-Slack-Api-App-Id"), Some("A01APP"), "{kind}");
    }
}

#[tokio::test]
async fn socket_event_carries_subtype_and_channel_type() {
    let publisher = MockJetStreamPublisher::new();
    let payload = r#"{"type":"event_callback","event_id":"Ev01ABC123","team_id":"T01ABC","event":{"type":"message","subtype":"bot_message","channel_type":"im"}}"#;

    handle_text_frame(&bridge(publisher.clone()), &envelope_text("events_api", payload))
        .await
        .unwrap();

    let messages = publisher.published_messages();
    let header = |name: &str| messages[0].headers.get(name).map(|v| v.as_str());
    assert_eq!(header("X-Slack-Event-Subtype"), Some("bot_message"));
    assert_eq!(header("X-Slack-Channel-Type"), Some("im"));
}

#[tokio::test]
async fn socket_delivery_carries_the_envelope_id_and_no_request_timestamp() {
    let publisher = MockJetStreamPublisher::new();
    let payload = r#"{"type":"event_callback","event_id":"Ev01ABC123","team_id":"T01ABC","event":{"type":"message"}}"#;

    handle_text_frame(&bridge(publisher.clone()), &envelope_text("events_api", payload))
        .await
        .unwrap();

    let messages = publisher.published_messages();
    let header = |name: &str| messages[0].headers.get(name).map(|v| v.as_str());
    assert_eq!(header("X-Slack-Envelope-Id"), Some("env-raw"));
    assert_eq!(header("X-Slack-Request-Timestamp"), None);
}

#[tokio::test]
async fn socket_envelope_id_with_line_break_is_unroutable_without_ack() {
    let publisher = MockJetStreamPublisher::new();
    let frame = serde_json::json!({
        "type": "events_api",
        "envelope_id": "env\r\n1",
        "payload": {
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message" }
        }
    })
    .to_string();

    let (_, ack) = handle_text_frame(&bridge(publisher.clone()), &frame).await.unwrap();

    assert_eq!(ack, None);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "slack.unroutable");
}

type Inbound = mpsc::UnboundedSender<Result<Message, WebSocketError>>;
type Outbound = mpsc::UnboundedReceiver<Message>;

struct SocketHarness {
    inbound: Inbound,
    outbound: Outbound,
    task: tokio::task::JoinHandle<Result<(), SocketModeError>>,
    started: tokio::time::Instant,
    reads: Arc<AtomicUsize>,
}

fn channel_sink(outbound: mpsc::UnboundedSender<Message>) -> impl Sink<Message, Error = WebSocketError> + Unpin {
    Box::pin(futures_util::sink::unfold(outbound, |outbound, message| async move {
        outbound.send(message).map_err(|_| WebSocketError::ConnectionClosed)?;
        Ok(outbound)
    }))
}

fn channel_stream(
    mut receiver: mpsc::UnboundedReceiver<Result<Message, WebSocketError>>,
    reads: Arc<AtomicUsize>,
) -> impl Stream<Item = Result<Message, WebSocketError>> + Unpin {
    Box::pin(futures_util::stream::poll_fn(move |cx| {
        let polled = receiver.poll_recv(cx);
        if matches!(polled, Poll::Ready(Some(_))) {
            reads.fetch_add(1, Ordering::SeqCst);
        }
        polled
    }))
}

fn start_socket() -> SocketHarness {
    start_socket_with(MockJetStreamPublisher::new())
}

fn start_socket_with<P: JetStreamPublisher>(publisher: P) -> SocketHarness {
    let (inbound, receiver) = mpsc::unbounded_channel();
    let (outbound_tx, outbound) = mpsc::unbounded_channel();
    let reads = Arc::new(AtomicUsize::new(0));
    let stream_reads = reads.clone();
    let task = tokio::spawn(async move {
        let mut receiver = channel_stream(receiver, stream_reads);
        let mut sender = channel_sink(outbound_tx);
        process_socket_messages(&bridge(publisher), &mut receiver, &mut sender, SOCKET_LIVENESS).await
    });
    SocketHarness {
        inbound,
        outbound,
        task,
        started: tokio::time::Instant::now(),
        reads,
    }
}

fn ping_time(count: u32) -> Duration {
    let nanos = (SOCKET_LIVENESS.ping_interval() * count).as_nanos();
    Duration::from_millis(nanos.div_ceil(1_000_000) as u64)
}

impl SocketHarness {
    async fn next_client_ping(&mut self) -> Duration {
        loop {
            match self.outbound.recv().await {
                Some(Message::Ping(payload)) => {
                    assert_eq!(payload.as_ref(), CLIENT_PING_PAYLOAD);
                    return self.started.elapsed();
                }
                Some(_) => {}
                None => panic!("socket closed before a client ping"),
            }
        }
    }

    fn pong(&self) {
        let _ = self.inbound.send(Ok(Message::Pong(Bytes::new())));
    }

    fn server_ping(&self) {
        self.inbound
            .send(Ok(Message::Ping(Bytes::from_static(b"server"))))
            .unwrap();
    }

    async fn answer_pings_until_closed(&mut self) {
        while let Some(message) = self.outbound.recv().await {
            if matches!(message, Message::Ping(_)) {
                self.pong();
            }
        }
    }

    async fn finish(self) -> Duration {
        let Self { task, started, .. } = self;
        task.await.unwrap().unwrap();
        started.elapsed()
    }
}

#[tokio::test(start_paused = true)]
async fn client_pings_are_sent_at_the_ping_interval() {
    let mut socket = start_socket();

    assert_eq!(socket.next_client_ping().await, ping_time(1));
    socket.pong();
    assert_eq!(socket.next_client_ping().await, ping_time(2));
    socket.pong();
    assert_eq!(socket.next_client_ping().await, ping_time(3));
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn connection_is_dropped_when_pings_never_get_a_pong() {
    let mut socket = start_socket();

    for count in 1..=4 {
        assert_eq!(socket.next_client_ping().await, ping_time(count));
    }

    assert_eq!(socket.finish().await, ping_time(4));
}

#[tokio::test(start_paused = true)]
async fn connection_is_dropped_a_timeout_after_a_ping_goes_unanswered() {
    let mut socket = start_socket();

    assert_eq!(socket.next_client_ping().await, ping_time(1));
    socket.pong();
    let unanswered = socket.next_client_ping().await;

    assert_eq!(unanswered, ping_time(2));
    assert_eq!(socket.finish().await, unanswered + CLIENT_PING_TIMEOUT);
}

#[derive(Clone)]
struct SlowPublisher {
    inner: MockJetStreamPublisher,
    delay: Duration,
}

impl JetStreamPublisher for SlowPublisher {
    type PublishError = <MockJetStreamPublisher as JetStreamPublisher>::PublishError;
    type AckFuture = <MockJetStreamPublisher as JetStreamPublisher>::AckFuture;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        tokio::time::sleep(self.delay).await;
        self.inner.publish_with_headers(subject, headers, payload).await
    }
}

#[tokio::test(start_paused = true)]
async fn slow_publishes_do_not_make_a_queued_pong_count_as_missing() {
    let publish_delay = CLIENT_PING_TIMEOUT / 2;
    let deliveries = 3;
    let mut socket = start_socket_with(SlowPublisher {
        inner: MockJetStreamPublisher::new(),
        delay: publish_delay,
    });
    socket.next_client_ping().await;
    socket.pong();
    socket.next_client_ping().await;
    for index in 0..deliveries {
        socket
            .inbound
            .send(Ok(socket_delivery(&format!("env-{index}"), &format!("EvSlow{index}"))))
            .unwrap();
    }
    socket.pong();

    let mut acks = 0;
    while acks < deliveries {
        match socket.outbound.recv().await {
            Some(Message::Text(_)) => acks += 1,
            Some(_) => {}
            None => panic!("connection closed while a pong was queued"),
        }
    }

    socket.next_client_ping().await;
    assert!(!socket.task.is_finished());
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn slow_publishes_before_the_first_pong_do_not_make_it_count_as_missing() {
    let publish_delay = CLIENT_PING_TIMEOUT / 2;
    let deliveries = 5;
    let mut socket = start_socket_with(SlowPublisher {
        inner: MockJetStreamPublisher::new(),
        delay: publish_delay,
    });
    socket.next_client_ping().await;
    for index in 0..deliveries {
        socket
            .inbound
            .send(Ok(socket_delivery(&format!("env-{index}"), &format!("EvSlow{index}"))))
            .unwrap();
    }
    socket.pong();

    let mut acks = 0;
    while acks < deliveries {
        match socket.outbound.recv().await {
            Some(Message::Text(_)) => acks += 1,
            Some(_) => {}
            None => panic!("connection closed while the first pong was queued"),
        }
    }

    socket.next_client_ping().await;
    assert!(!socket.task.is_finished());
    socket.task.abort();
}

fn socket_delivery(envelope_id: &str, event_id: &str) -> Message {
    let payload = format!(
        r#"{{"type":"event_callback","event_id":"{event_id}","team_id":"T01ABC","event":{{"type":"message"}}}}"#
    );
    let frame = format!(r#"{{"type":"events_api","envelope_id":"{envelope_id}","payload":{payload}}}"#);
    Message::Text(frame.into())
}

#[tokio::test(start_paused = true)]
async fn answered_pings_keep_the_connection_open() {
    let mut socket = start_socket();

    for _ in 0..30 {
        socket.next_client_ping().await;
        socket.pong();
    }

    assert!(!socket.task.is_finished());
    socket.inbound.send(Ok(Message::Close(None))).unwrap();
    socket.finish().await;
}

#[tokio::test(start_paused = true)]
async fn missing_server_pings_do_not_stall_while_the_server_answers_pings() {
    let mut socket = start_socket();

    while socket.started.elapsed() < SERVER_PING_TIMEOUT * 2 {
        socket.next_client_ping().await;
        socket.pong();
    }

    assert!(!socket.task.is_finished());
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn server_ping_is_answered_with_a_pong() {
    let mut socket = start_socket();

    socket.server_ping();

    assert!(matches!(socket.outbound.recv().await, Some(Message::Pong(payload)) if payload.as_ref() == b"server"));
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn connection_is_dropped_when_server_pings_stop() {
    let mut socket = start_socket();
    socket.server_ping();

    socket.answer_pings_until_closed().await;

    assert_eq!(socket.finish().await, SERVER_PING_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn each_server_ping_restarts_the_server_ping_timeout() {
    let mut socket = start_socket();
    socket.server_ping();
    while socket.started.elapsed() < SERVER_PING_TIMEOUT - CLIENT_PING_TIMEOUT {
        socket.next_client_ping().await;
        socket.pong();
    }
    let second_ping_at = socket.started.elapsed();
    socket.server_ping();

    socket.answer_pings_until_closed().await;

    assert_eq!(socket.finish().await, second_ping_at + SERVER_PING_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn client_ping_send_failure_ends_the_connection() {
    let SocketHarness {
        task,
        outbound,
        inbound,
        ..
    } = start_socket();
    drop(outbound);

    let error = task.await.unwrap().unwrap_err();
    drop(inbound);

    assert!(matches!(
        error,
        SocketModeError::WebSocket(WebSocketError::ConnectionClosed)
    ));
}

#[test]
fn only_credential_and_link_failures_are_permanent() {
    let api = |code: &str| SocketModeError::Api(ConnectionsOpenErrorCode::new(code));

    assert!(api("invalid_auth").is_permanent());
    assert!(!api("ratelimited").is_permanent());
    assert!(SocketModeError::LinkDisabled.is_permanent());
    assert!(!SocketModeError::MissingSocketModeConfig.is_permanent());
    assert!(!SocketModeError::WebSocket(WebSocketError::ConnectionClosed).is_permanent());
    assert_eq!(
        SocketModeError::LinkDisabled.to_string(),
        "Slack disabled Socket Mode for this app"
    );
}

#[tokio::test(start_paused = true)]
async fn permanent_open_errors_keep_the_existing_backoff() {
    let (attempts, mut observed) = mpsc::channel(1);
    let connector = ScriptedConnector {
        results: [
            Err(SocketModeError::Api(ConnectionsOpenErrorCode::new("invalid_auth"))),
            Err(SocketModeError::LinkDisabled),
        ]
        .into(),
        attempts,
    };
    let task = tokio::spawn(run(
        wrap_publisher(MockJetStreamPublisher::new()),
        AdvancedMockNatsClient::new(),
        socket_config(),
        connector,
    ));
    let first = observed.recv().await.unwrap();
    let second = observed.recv().await.unwrap();
    let third = observed.recv().await.unwrap();
    assert_eq!(second - first, RECONNECT_INITIAL_DELAY);
    assert_eq!(third - second, RECONNECT_INITIAL_DELAY * 2);
    task.abort();
}

#[tokio::test]
async fn malformed_frames_are_skipped_without_publishing() {
    let publisher = MockJetStreamPublisher::new();
    for frame in ["not json", "{", r#"{"envelope_id":"env-1"}"#, r#"{"type":7}"#, ""] {
        let (reconnect, ack) = handle_text_frame(&bridge(publisher.clone()), frame).await.unwrap();

        assert!(!reconnect, "{frame}");
        assert_eq!(ack, None, "{frame}");
    }
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_malformed_frame_does_not_close_the_connection() {
    let publisher = MockJetStreamPublisher::new();
    let mut socket = start_socket_with(publisher.clone());
    let envelope = envelope_text(
        "events_api",
        r#"{"type":"event_callback","event_id":"EvAfterJunk","team_id":"T01ABC","event":{"type":"message"}}"#,
    );

    socket.inbound.send(Ok(Message::Text("{not json".into()))).unwrap();
    socket.inbound.send(Ok(Message::Text(envelope.into()))).unwrap();

    let ack = loop {
        match socket.outbound.recv().await {
            Some(Message::Text(ack)) => break ack,
            Some(_) => {}
            None => panic!("connection closed after a malformed frame"),
        }
    };
    assert_eq!(ack.as_str(), r#"{"envelope_id":"env-raw"}"#);
    assert!(!socket.task.is_finished());
    socket.task.abort();
}

#[tokio::test]
async fn link_disabled_disconnect_is_reported_as_a_permanent_failure() {
    let envelope = serde_json::json!({ "type": "disconnect", "reason": "link_disabled" });

    let error = handle_text_frame(&bridge(MockJetStreamPublisher::new()), &envelope.to_string())
        .await
        .unwrap_err();

    assert!(matches!(error, SocketModeError::LinkDisabled));
}

const SLOW_MARKER: &str = "EvSlow";
const SLOW_PUBLISH: Duration = Duration::from_secs(2);
const FAST_PUBLISH: Duration = Duration::from_secs(1);

#[derive(Clone)]
struct DelayedPublisher {
    inner: MockJetStreamPublisher,
    fast: Duration,
    slow: Duration,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl DelayedPublisher {
    fn new(fast: Duration, slow: Duration) -> Self {
        Self {
            inner: MockJetStreamPublisher::new(),
            fast,
            slow,
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

struct ActivePublish(Arc<AtomicUsize>);

impl Drop for ActivePublish {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl JetStreamPublisher for DelayedPublisher {
    type PublishError = <MockJetStreamPublisher as JetStreamPublisher>::PublishError;
    type AckFuture = <MockJetStreamPublisher as JetStreamPublisher>::AckFuture;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let _guard = ActivePublish(self.active.clone());
        let slow = payload
            .windows(SLOW_MARKER.len())
            .any(|window| window == SLOW_MARKER.as_bytes());
        tokio::time::sleep(if slow { self.slow } else { self.fast }).await;
        self.inner.publish_with_headers(subject, headers, payload).await
    }
}

fn envelope_id_of(ack: &str) -> String {
    serde_json::from_str::<serde_json::Value>(ack).unwrap()["envelope_id"]
        .as_str()
        .unwrap()
        .to_string()
}

impl SocketHarness {
    fn send(&self, message: Message) {
        self.inbound.send(Ok(message)).unwrap();
    }

    async fn next_ack(&mut self) -> (String, Duration) {
        loop {
            match self.outbound.recv().await {
                Some(Message::Text(ack)) => return (envelope_id_of(ack.as_str()), self.started.elapsed()),
                Some(_) => {}
                None => panic!("socket closed before an ack"),
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_slow_publish_does_not_delay_the_acks_of_other_envelopes() {
    let mut socket = start_socket_with(DelayedPublisher::new(Duration::ZERO, SLOW_PUBLISH));
    socket.send(socket_delivery("env-slow", "EvSlow1"));
    for index in 1..=3 {
        socket.send(socket_delivery(&format!("env-fast-{index}"), &format!("EvFast{index}")));
    }

    let mut acks = Vec::new();
    for _ in 0..4 {
        acks.push(socket.next_ack().await);
    }

    let mut fast: Vec<_> = acks[..3].iter().map(|(id, at)| (id.clone(), *at)).collect();
    fast.sort();
    assert_eq!(
        fast,
        (1..=3)
            .map(|index| (format!("env-fast-{index}"), Duration::ZERO))
            .collect::<Vec<_>>()
    );
    assert_eq!(acks[3], ("env-slow".to_string(), SLOW_PUBLISH));
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn envelopes_over_the_limit_wait_for_a_free_slot_instead_of_being_dropped() {
    let publisher = DelayedPublisher::new(FAST_PUBLISH, FAST_PUBLISH);
    let mut socket = start_socket_with(publisher.clone());
    let total = ENVELOPE_CONCURRENCY_LIMIT + 4;
    for index in 0..total {
        socket.send(socket_delivery(&format!("env-{index}"), &format!("EvFast{index}")));
    }

    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(socket.reads.load(Ordering::SeqCst), ENVELOPE_CONCURRENCY_LIMIT);

    let mut acks = Vec::new();
    for _ in 0..total {
        acks.push(socket.next_ack().await);
    }

    let mut ids: Vec<_> = acks.iter().map(|(id, _)| id.clone()).collect();
    ids.sort();
    let mut expected: Vec<_> = (0..total).map(|index| format!("env-{index}")).collect();
    expected.sort();
    assert_eq!(ids, expected);
    assert!(
        acks[..ENVELOPE_CONCURRENCY_LIMIT]
            .iter()
            .all(|(_, at)| *at == FAST_PUBLISH)
    );
    assert!(
        acks[ENVELOPE_CONCURRENCY_LIMIT..]
            .iter()
            .all(|(_, at)| *at == FAST_PUBLISH * 2)
    );
    assert_eq!(publisher.peak(), ENVELOPE_CONCURRENCY_LIMIT);
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_queued_pong_behind_a_full_set_of_slow_publishes_does_not_count_as_missing() {
    let mut socket = start_socket_with(DelayedPublisher::new(SLOW_PUBLISH, SLOW_PUBLISH));
    socket.next_client_ping().await;
    socket.pong();
    let unanswered = socket.next_client_ping().await;
    for index in 0..ENVELOPE_CONCURRENCY_LIMIT {
        socket.send(socket_delivery(&format!("env-{index}"), &format!("EvSlow{index}")));
    }
    socket.pong();

    for _ in 0..ENVELOPE_CONCURRENCY_LIMIT {
        let (_, at) = socket.next_ack().await;
        assert_eq!(at, unanswered + SLOW_PUBLISH);
    }

    while socket.started.elapsed() < unanswered + CLIENT_PING_TIMEOUT * 2 {
        socket.next_client_ping().await;
        socket.pong();
    }
    assert!(!socket.task.is_finished());
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_first_pong_behind_a_full_set_of_slow_publishes_does_not_count_as_missing() {
    let mut socket = start_socket_with(DelayedPublisher::new(SLOW_PUBLISH, SLOW_PUBLISH));
    let first = socket.next_client_ping().await;
    for index in 0..ENVELOPE_CONCURRENCY_LIMIT {
        socket.send(socket_delivery(&format!("env-{index}"), &format!("EvSlow{index}")));
    }
    socket.pong();

    for _ in 0..ENVELOPE_CONCURRENCY_LIMIT {
        socket.next_ack().await;
    }

    while socket.started.elapsed() < first + CLIENT_PING_INTERVAL * UNANSWERED_PING_LIMIT * 2 {
        socket.next_client_ping().await;
        socket.pong();
    }
    assert!(!socket.task.is_finished());
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn pings_keep_flowing_while_an_envelope_is_in_flight() {
    let mut socket = start_socket_with(DelayedPublisher::new(Duration::ZERO, SLOW_PUBLISH));
    socket.send(socket_delivery("env-slow", "EvSlow1"));

    assert_eq!(socket.next_client_ping().await, ping_time(1));
    socket.pong();
    socket.server_ping();

    assert_eq!(socket.next_ack().await, ("env-slow".to_string(), SLOW_PUBLISH));
    assert!(!socket.task.is_finished());
    socket.task.abort();
}

#[tokio::test(start_paused = true)]
async fn disconnect_lets_in_flight_envelopes_finish_and_ack() {
    let mut socket = start_socket_with(DelayedPublisher::new(Duration::ZERO, SLOW_PUBLISH));
    socket.send(socket_delivery("env-slow", "EvSlow1"));
    socket.send(Message::Text(
        r#"{"type":"disconnect","reason":"refresh_requested"}"#.into(),
    ));

    assert_eq!(socket.next_ack().await, ("env-slow".to_string(), SLOW_PUBLISH));
    assert_eq!(socket.finish().await, SLOW_PUBLISH);
}

#[tokio::test(start_paused = true)]
async fn disconnect_drain_gives_up_when_an_ack_cannot_be_written() {
    let socket = start_socket_with(DelayedPublisher::new(Duration::ZERO, SLOW_PUBLISH));
    socket.send(socket_delivery("env-slow-1", "EvSlow1"));
    socket.send(socket_delivery("env-slow-2", "EvSlow2"));
    socket.send(Message::Text(r#"{"type":"disconnect"}"#.into()));
    let SocketHarness { task, outbound, .. } = socket;
    drop(outbound);

    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn close_lets_in_flight_envelopes_finish_publishing() {
    let publisher = DelayedPublisher::new(Duration::ZERO, SLOW_PUBLISH);
    let mut socket = start_socket_with(publisher.clone());
    socket.send(socket_delivery("env-slow", "EvSlow1"));
    socket.send(Message::Close(None));

    assert_eq!(socket.next_ack().await, ("env-slow".to_string(), SLOW_PUBLISH));
    assert_eq!(socket.finish().await, SLOW_PUBLISH);
    assert_eq!(publisher.inner.published_messages().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn stream_end_lets_in_flight_envelopes_finish_publishing() {
    let publisher = DelayedPublisher::new(Duration::ZERO, SLOW_PUBLISH);
    let socket = start_socket_with(publisher.clone());
    socket.send(socket_delivery("env-slow", "EvSlow1"));
    let SocketHarness {
        inbound,
        mut outbound,
        task,
        started,
        ..
    } = socket;
    drop(inbound);

    task.await.unwrap().unwrap();

    assert_eq!(started.elapsed(), SLOW_PUBLISH);
    assert_eq!(publisher.inner.published_messages().len(), 1);
    assert!(matches!(outbound.recv().await, Some(Message::Text(_))));
}

#[tokio::test(start_paused = true)]
async fn link_disabled_lets_in_flight_envelopes_finish_before_failing() {
    let publisher = DelayedPublisher::new(Duration::ZERO, SLOW_PUBLISH);
    let mut socket = start_socket_with(publisher.clone());
    socket.send(socket_delivery("env-slow", "EvSlow1"));
    socket.send(Message::Text(
        r#"{"type":"disconnect","reason":"link_disabled"}"#.into(),
    ));

    assert_eq!(socket.next_ack().await, ("env-slow".to_string(), SLOW_PUBLISH));
    let SocketHarness { task, started, .. } = socket;
    let error = task.await.unwrap().unwrap_err();

    assert!(matches!(error, SocketModeError::LinkDisabled));
    assert_eq!(started.elapsed(), SLOW_PUBLISH);
    assert_eq!(publisher.inner.published_messages().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_delivery_that_is_not_acked_leaves_the_connection_open() {
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let mut socket = start_socket_with(publisher.clone());
    socket.send(socket_delivery("env-failed", "EvFailed1"));
    socket.send(socket_delivery("env-stored", "EvStored1"));

    let (acked, _) = socket.next_ack().await;

    assert_eq!(acked, "env-stored");
    assert_eq!(publisher.published_messages().len(), 1);
    assert!(!socket.task.is_finished());
    socket.task.abort();
}

#[tokio::test]
async fn socket_slash_command_reply_is_the_ack_payload_byte_for_byte() {
    let requester = replying_requester(
        "slack.respond.command.trogon",
        Some("application/json"),
        br#"{ "response_type":"in_channel",  "text":"hi" }"#,
    );
    let publisher = MockJetStreamPublisher::new();

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(publisher.clone(), requester.clone()),
        &slash_envelope(Some(true)),
    )
    .await
    .unwrap();

    assert_eq!(
        ack.as_deref(),
        Some(r#"{"envelope_id":"env-3","payload":{ "response_type":"in_channel",  "text":"hi" }}"#)
    );
    assert_eq!(publisher.published_subjects(), vec!["slack.command.trogon"]);
    assert_eq!(requester.requested_payloads().len(), 1);
}

#[tokio::test]
async fn socket_text_reply_is_wrapped_as_the_text_field() {
    let requester = replying_requester(
        "slack.respond.command.trogon",
        Some("text/plain; charset=utf-8"),
        b"say \"hi\"",
    );

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(MockJetStreamPublisher::new(), requester),
        &slash_envelope(None),
    )
    .await
    .unwrap();

    assert_eq!(
        ack.as_deref(),
        Some(r#"{"envelope_id":"env-3","payload":{"text":"say \"hi\""}}"#)
    );
}

#[tokio::test]
async fn socket_view_submission_reply_carries_response_action() {
    let requester = replying_requester(
        "slack.respond.interaction.view_submission",
        Some("application/json"),
        br#"{"response_action":"clear"}"#,
    );

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(MockJetStreamPublisher::new(), requester.clone()),
        &view_submission_envelope(),
    )
    .await
    .unwrap();

    assert_eq!(
        ack.as_deref(),
        Some(r#"{"envelope_id":"env-4","payload":{"response_action":"clear"}}"#)
    );
    let headers = requester.requested_headers();
    assert_eq!(
        headers[0].get("X-Slack-Callback-Id").map(|v| v.as_str()),
        Some("my_modal")
    );
}

#[tokio::test]
async fn an_empty_socket_reply_is_a_plain_ack() {
    let (_, ack) = handle_text_frame(&bridge(MockJetStreamPublisher::new()), &slash_envelope(None))
        .await
        .unwrap();

    assert_eq!(ack.as_deref(), Some(r#"{"envelope_id":"env-3"}"#));
}

#[tokio::test]
async fn socket_replies_that_are_not_json_are_acknowledged_without_a_payload() {
    for (content_type, body) in [
        (Some("application/json"), &b"not json"[..]),
        (Some("text/plain"), &[0xff, 0xfe][..]),
    ] {
        let requester = AdvancedMockNatsClient::new();
        let mut headers = async_nats::HeaderMap::new();
        if let Some(content_type) = content_type {
            headers.insert("Content-Type", content_type);
        }
        requester.set_response_wire("slack.respond.command.trogon", headers, Bytes::copy_from_slice(body));

        let (_, ack) = handle_text_frame(
            &bridge_with_requester(MockJetStreamPublisher::new(), requester),
            &slash_envelope(None),
        )
        .await
        .unwrap();

        assert_eq!(ack.as_deref(), Some(r#"{"envelope_id":"env-3"}"#));
    }
}

#[tokio::test]
async fn no_consumer_answering_sends_no_socket_frame() {
    let publisher = MockJetStreamPublisher::new();
    let (_, ack) = handle_text_frame(
        &bridge_with_requester(publisher.clone(), AdvancedMockNatsClient::new()),
        &slash_envelope(None),
    )
    .await
    .unwrap();

    assert_eq!(ack, None);
    assert_eq!(publisher.published_subjects(), vec!["slack.command.trogon"]);
}

#[tokio::test]
async fn a_failed_consumer_request_sends_no_socket_frame() {
    let requester = acknowledging_requester();
    requester.fail_next_request();

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(MockJetStreamPublisher::new(), requester),
        &view_submission_envelope(),
    )
    .await
    .unwrap();

    assert_eq!(ack, None);
}

#[tokio::test(start_paused = true)]
async fn a_consumer_that_misses_the_deadline_sends_no_socket_frame() {
    let requester = acknowledging_requester();
    requester.hang_next_request();

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(MockJetStreamPublisher::new(), requester),
        &slash_envelope(None),
    )
    .await
    .unwrap();

    assert_eq!(ack, None);
}

#[tokio::test]
async fn an_envelope_that_refuses_response_payloads_is_acked_without_asking_a_consumer() {
    let requester = replying_requester("slack.respond.command.trogon", Some("application/json"), b"{}");
    let publisher = MockJetStreamPublisher::new();

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(publisher.clone(), requester.clone()),
        &slash_envelope(Some(false)),
    )
    .await
    .unwrap();

    assert_eq!(ack.as_deref(), Some(r#"{"envelope_id":"env-3"}"#));
    assert_eq!(publisher.published_subjects(), vec!["slack.command.trogon"]);
    assert!(requester.requested_payloads().is_empty());
}

#[tokio::test]
async fn socket_block_suggestion_options_are_the_ack_payload_and_nothing_is_published() {
    let requester = replying_requester(
        "slack.respond.interaction.block_suggestion",
        Some("application/json"),
        br#"{"options":[]}"#,
    );
    let publisher = MockJetStreamPublisher::new();

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(publisher.clone(), requester.clone()),
        &block_suggestion_envelope(true),
    )
    .await
    .unwrap();

    assert_eq!(
        ack.as_deref(),
        Some(r#"{"envelope_id":"env-5","payload":{"options":[]}}"#)
    );
    assert!(publisher.published_messages().is_empty());
    let headers = requester.requested_headers();
    assert_eq!(headers[0].get("X-Slack-Action-Id").map(|v| v.as_str()), Some("pick"));
    assert_eq!(headers[0].get("X-Slack-Block-Id").map(|v| v.as_str()), Some("blk"));
}

#[tokio::test]
async fn socket_block_suggestion_with_no_consumer_sends_no_frame() {
    let publisher = MockJetStreamPublisher::new();
    let (_, ack) = handle_text_frame(
        &bridge_with_requester(publisher.clone(), AdvancedMockNatsClient::new()),
        &block_suggestion_envelope(true),
    )
    .await
    .unwrap();

    assert_eq!(ack, None);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn socket_block_suggestion_that_refuses_a_payload_is_acked_without_asking() {
    let requester = replying_requester(
        "slack.respond.interaction.block_suggestion",
        Some("application/json"),
        br#"{"options":[]}"#,
    );

    let (_, ack) = handle_text_frame(
        &bridge_with_requester(MockJetStreamPublisher::new(), requester.clone()),
        &block_suggestion_envelope(false),
    )
    .await
    .unwrap();

    assert_eq!(ack.as_deref(), Some(r#"{"envelope_id":"env-5"}"#));
    assert!(requester.requested_payloads().is_empty());
}
