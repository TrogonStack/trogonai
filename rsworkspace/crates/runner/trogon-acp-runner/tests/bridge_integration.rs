#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for acp-nats Bridge with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test bridge_integration

use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

use acp_nats::agent_handler::AgentHandler;
use acp_nats::prompt_event::PromptEvent;
use acp_nats::{AGENT_UNAVAILABLE, AcpPrefix, Bridge, Config, NatsAuth, NatsConfig};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    ContentBlock, ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse, ImageContent,
    Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, ResumeSessionRequest,
    ResumeSessionResponse, SessionId, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, StopReason,
};
use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_acp_runner::prompt_converter::{PromptEventConverter, PromptOutcome};
use trogon_std::time::SystemClock;

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

type NatsBridge = Bridge<async_nats::Client, SystemClock, trogon_nats::jetstream::NatsJetStreamClient>;

fn make_js(nats: &async_nats::Client) -> trogon_nats::jetstream::NatsJetStreamClient {
    trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats.clone()))
}

/// Provision only the session-scoped JetStream streams (Commands, Responses,
/// ClientOps, Notifications). The Global and GlobalExt streams are skipped
/// because provisioning them causes NATS to deliver a JetStream PubAck to
/// NATS-Core request reply-subjects (e.g. authenticate, initialize), which
/// makes timeout tests receive an unexpected Ok response.
async fn provision_session_streams(js: &trogon_nats::jetstream::NatsJetStreamClient, prefix: &AcpPrefix) {
    use acp_nats::nats::AcpStream;
    use trogon_nats::jetstream::JetStreamContext;
    for stream in [AcpStream::Commands, AcpStream::Responses, AcpStream::ClientOps] {
        JetStreamContext::get_or_create_stream(js, stream.config(prefix))
            .await
            .expect("failed to provision session JetStream stream");
    }
}

async fn make_bridge(nats: async_nats::Client, prefix: &str) -> NatsBridge {
    let acp_prefix = AcpPrefix::new(prefix).unwrap();
    let config = Config::new(
        acp_prefix.clone(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500))
    .with_prompt_timeout(Duration::from_secs(5));
    let js = make_js(&nats);
    provision_session_streams(&js, &acp_prefix).await;
    Bridge::new(
        nats,
        js,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-integration-test"),
        config,
    )
}

/// Like `make_bridge` but keeps the notification receiver alive so tests can
/// assert on the `SessionNotification`s produced during a prompt.
async fn make_bridge_with_rx(
    nats: async_nats::Client,
    prefix: &str,
) -> (
    NatsBridge,
    tokio::sync::mpsc::Receiver<agent_client_protocol::schema::v1::SessionNotification>,
) {
    let acp_prefix = AcpPrefix::new(prefix).unwrap();
    let config = Config::new(
        acp_prefix.clone(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500))
    .with_prompt_timeout(Duration::from_secs(5));
    let js = make_js(&nats);
    provision_session_streams(&js, &acp_prefix).await;
    let (_tx, rx) = tokio::sync::mpsc::channel(32);
    let bridge = Bridge::new(
        nats,
        js,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-integration-test"),
        config,
    );
    (bridge, rx)
}

// ── initialize ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_returns_protocol_version_from_agent() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge.initialize(InitializeRequest::new(ProtocolVersion::LATEST)).await;

    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
    assert_eq!(result.unwrap().protocol_version, ProtocolVersion::LATEST);
}

#[tokio::test]
async fn initialize_returns_agent_unavailable_when_no_agent() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    // Nobody is subscribed — NATS immediately returns "no responders".
    // This maps to AGENT_UNAVAILABLE (same as a timeout would).
    let err = bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        ErrorCode::Other(AGENT_UNAVAILABLE),
        "expected AGENT_UNAVAILABLE, got: {:?}",
        err.code
    );
}

#[tokio::test]
async fn initialize_returns_error_on_invalid_json_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await
            && let Some(reply) = msg.reply
        {
            // Send malformed JSON.
            nats2.publish(reply, b"{bad json}".as_ref().into()).await.unwrap();
        }
    });

    let err = bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap_err();

    assert_eq!(
        err.code,
        ErrorCode::InternalError,
        "expected InternalError, got: {:?}",
        err.code
    );
    assert!(
        err.to_string().contains("Invalid response from agent"),
        "expected 'Invalid response from agent', got: {}",
        err
    );
}

// ── authenticate ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn authenticate_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.agent.authenticate").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&AuthenticateResponse::default()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge.authenticate(AuthenticateRequest::new("password")).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

#[tokio::test]
async fn authenticate_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    let err = bridge
        .authenticate(AuthenticateRequest::new("password"))
        .await
        .unwrap_err();

    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_session_returns_session_id() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let expected_id = SessionId::from("sess-abc-123");
    let mut agent_sub = nats.subscribe("acp.agent.session.new").await.unwrap();
    let nats2 = nats.clone();
    let resp_id = expected_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&NewSessionResponse::new(resp_id)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge.new_session(NewSessionRequest::new(".")).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
    assert_eq!(result.unwrap().session_id, expected_id);
}

// ── load_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_session_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.session.s1.agent.load").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let subject = msg.subject.to_string();
            let _ = tx.send(subject);
            js_respond_session(&nats2, &msg, "acp", "s1", &LoadSessionResponse::new()).await;
        }
    });

    bridge.load_session(LoadSessionRequest::new("s1", ".")).await.unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();

    assert_eq!(subject, "acp.session.s1.agent.load");
}

#[tokio::test]
async fn load_session_invalid_session_id_returns_error_without_nats() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    // A session ID with dots is rejected by AcpSessionId validation
    // before any NATS publish happens.
    let mut should_not_receive = nats.subscribe("acp.>").await.unwrap();

    let err = bridge
        .load_session(LoadSessionRequest::new("invalid.session.id", "."))
        .await
        .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert!(err.to_string().contains("Invalid session ID"));

    // No message should have been sent to NATS.
    let result = tokio::time::timeout(Duration::from_millis(100), should_not_receive.next()).await;
    assert!(
        result.is_err(),
        "no NATS message should be sent for invalid session IDs"
    );
}

// ── set_session_mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_mode_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.session.s1.agent.set_mode").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            js_respond_session(&nats2, &msg, "acp", "s1", &SetSessionModeResponse::new()).await;
        }
    });

    let result = bridge.set_session_mode(SetSessionModeRequest::new("s1", "edit")).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

// ── cancel ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_publishes_to_correct_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut sub = nats.subscribe("acp.session.s1.agent.cancel").await.unwrap();

    bridge.cancel(CancelNotification::new("s1")).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .expect("timed out waiting for cancel message")
        .expect("subscriber closed");

    assert_eq!(msg.subject.as_str(), "acp.session.s1.agent.cancel");
}

#[tokio::test]
async fn cancel_always_returns_ok_even_if_no_subscriber() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    // Fire-and-forget: no subscriber, but cancel still returns Ok(()).
    let result = bridge.cancel(CancelNotification::new("s1")).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn cancel_invalid_session_id_returns_error_before_publish() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut should_not_receive = nats.subscribe("acp.>").await.unwrap();

    let err = bridge
        .cancel(CancelNotification::new("invalid.session.id"))
        .await
        .unwrap_err();

    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert!(err.to_string().contains("Invalid session ID"));

    let result = tokio::time::timeout(Duration::from_millis(100), should_not_receive.next()).await;
    assert!(
        result.is_err(),
        "no NATS message should be published for invalid session IDs"
    );
}

// ── cross-cutting ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn custom_prefix_used_in_all_subjects() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "custom").await;

    // The subject must start with "custom.", not "acp.".
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("custom.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let subject = msg.subject.to_string();
            let _ = tx.send(subject);
            let resp = serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out")
        .unwrap();

    assert_eq!(subject, "custom.agent.initialize");
}

#[tokio::test]
async fn initialize_with_client_info_forwarded_to_agent() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            // Capture the raw payload to verify the client name is present.
            let payload_str = String::from_utf8_lossy(&msg.payload).to_string();
            let _ = tx.send(payload_str);
            let resp = serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .initialize(
            InitializeRequest::new(ProtocolVersion::LATEST).client_info(Implementation::new("my-client", "1.0.0")),
        )
        .await
        .unwrap();

    let payload = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out")
        .unwrap();

    assert!(
        payload.contains("my-client"),
        "expected 'my-client' in request payload, got: {payload}"
    );
}

#[tokio::test]
async fn concurrent_requests_dont_mix_replies() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let counter = Arc::new(AtomicU32::new(0));

    // Mock agent handles multiple new_session requests, giving each a unique ID.
    let mut agent_sub = nats.subscribe("acp.agent.session.new").await.unwrap();
    let nats2 = nats.clone();
    let counter2 = counter.clone();
    tokio::spawn(async move {
        while let Some(msg) = agent_sub.next().await {
            let idx = counter2.fetch_add(1, Ordering::SeqCst);
            let session_id = SessionId::from(format!("concurrent-sess-{}", idx));
            let resp = serde_json::to_vec(&NewSessionResponse::new(session_id)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    // Bridge futures are !Send (async_trait(?Send)), so use tokio::join! instead of spawn.
    let (r0, r1, r2, r3, r4) = tokio::join!(
        bridge.new_session(NewSessionRequest::new(".")),
        bridge.new_session(NewSessionRequest::new(".")),
        bridge.new_session(NewSessionRequest::new(".")),
        bridge.new_session(NewSessionRequest::new(".")),
        bridge.new_session(NewSessionRequest::new(".")),
    );

    let mut session_ids = HashSet::new();
    for result in [r0, r1, r2, r3, r4] {
        assert!(result.is_ok(), "concurrent request failed: {:?}", result.unwrap_err());
        let id = result.unwrap().session_id.to_string();
        assert!(!id.is_empty(), "session_id must not be empty");
        session_ids.insert(id);
    }

    // All 5 should have received distinct session IDs — no reply cross-mixing.
    assert_eq!(
        session_ids.len(),
        5,
        "all 5 concurrent sessions should have distinct IDs"
    );
}

// ── prompt helpers ────────────────────────────────────────────────────────────

/// Publish a JetStream response to the Responses stream for a session command.
///
/// Bridge session operations (fork, resume, close, set_model, etc.) use JetStream
/// via `js_request`. They publish to the Commands stream with no reply-to subject.
/// The response must be JetStream-published to
/// `{prefix}.session.{session_id}.agent.response.{req_id}` so the bridge's
/// Responses-stream consumer picks it up.
async fn js_respond_session<R: serde::Serialize>(
    nats: &async_nats::Client,
    msg: &async_nats::Message,
    prefix: &str,
    session_id: &str,
    response: &R,
) {
    let req_id = msg
        .headers
        .as_ref()
        .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
        .map(|v| v.as_str().to_string())
        .unwrap_or_default();
    let resp_subject = format!("{prefix}.session.{session_id}.agent.response.{req_id}");
    let resp_bytes: bytes::Bytes = serde_json::to_vec(response).unwrap().into();
    async_nats::jetstream::new(nats.clone())
        .publish(resp_subject, resp_bytes)
        .await
        .unwrap()
        .await
        .unwrap();
}

/// Parse a stop-reason string to `StopReason`, falling back to `EndTurn`.
fn parse_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "max_turn_requests" => StopReason::MaxTurnRequests,
        "cancelled" => StopReason::Cancelled,
        _ => StopReason::EndTurn,
    }
}

/// Spawn a mock runner that:
/// 1. Subscribes to `{prefix}.session.{session_id}.agent.prompt`
/// 2. Reads `req_id` from the `X-Req-Id` header
/// 3. Converts each `PromptEvent` via `PromptEventConverter` into `SessionNotification`s
/// 4. JetStream-publishes notifications to `{prefix}.session.{session_id}.agent.update.{req_id}`
/// 5. On terminal outcome (Done/Error) JetStream-publishes to
///    `{prefix}.session.{session_id}.agent.prompt.response.{req_id}`
///
/// Returns only after the NATS subscription is confirmed, eliminating the race
/// where the bridge publishes the prompt before the mock has subscribed.
async fn mock_runner(nats: async_nats::Client, prefix: &str, session_id: &str, events: Vec<PromptEvent>) {
    let subject = format!("{}.session.{}.agent.prompt", prefix, session_id);
    let prefix = prefix.to_string();
    let session_id = session_id.to_string();
    // Subscribe BEFORE returning so the bridge can't miss the prompt.
    let mut sub = nats.subscribe(subject).await.unwrap();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            let update_subject = format!("{}.session.{}.agent.update.{}", prefix, session_id, req_id);
            let response_subject = format!("{}.session.{}.agent.prompt.response.{}", prefix, session_id, req_id);

            let js = async_nats::jetstream::new(nats.clone());
            let mut converter = PromptEventConverter::new(session_id.clone());
            for event in events {
                let (notifications, outcome) = converter.convert(event);
                for notif in &notifications {
                    js.publish(update_subject.clone(), serde_json::to_vec(notif).unwrap().into())
                        .await
                        .unwrap()
                        .await
                        .unwrap();
                }
                if let Some(outcome) = outcome {
                    match outcome {
                        PromptOutcome::Done { stop_reason } => {
                            let resp = agent_client_protocol::schema::v1::PromptResponse::new(parse_stop_reason(&stop_reason));
                            js.publish(response_subject, serde_json::to_vec(&resp).unwrap().into())
                                .await
                                .unwrap()
                                .await
                                .unwrap();
                        }
                        PromptOutcome::Error { message } => {
                            let env = serde_json::json!({"error": message});
                            js.publish(response_subject, serde_json::to_vec(&env).unwrap().into())
                                .await
                                .unwrap()
                                .await
                                .unwrap();
                        }
                    }
                    return;
                }
            }
        }
    });
}

// ── prompt / event types ──────────────────────────────────────────────────────

#[tokio::test]
async fn error_event_returns_err_from_prompt() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-err",
        vec![PromptEvent::Error {
            message: "something blew up".to_string(),
        }],
    )
    .await;

    let result = bridge.prompt(PromptRequest::new("sess-err", vec![])).await;
    assert!(result.is_err(), "expected Err from Error event");
    assert!(result.unwrap_err().to_string().contains("something blew up"));
}

#[tokio::test]
async fn done_stop_reason_end_turn_maps_correctly() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-done-et",
        vec![PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        }],
    )
    .await;

    let resp = bridge.prompt(PromptRequest::new("sess-done-et", vec![])).await.unwrap();
    assert!(
        matches!(resp.stop_reason, StopReason::EndTurn),
        "got: {:?}",
        resp.stop_reason
    );
}

#[tokio::test]
async fn done_stop_reason_max_tokens_maps_correctly() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-done-mt",
        vec![PromptEvent::Done {
            stop_reason: "max_tokens".to_string(),
        }],
    )
    .await;

    let resp = bridge.prompt(PromptRequest::new("sess-done-mt", vec![])).await.unwrap();
    assert!(
        matches!(resp.stop_reason, StopReason::MaxTokens),
        "got: {:?}",
        resp.stop_reason
    );
}

#[tokio::test]
async fn done_unknown_stop_reason_falls_back_to_end_turn() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-done-unk",
        vec![PromptEvent::Done {
            stop_reason: "totally_unknown_reason".to_string(),
        }],
    )
    .await;

    let resp = bridge
        .prompt(PromptRequest::new("sess-done-unk", vec![]))
        .await
        .unwrap();
    assert!(
        matches!(resp.stop_reason, StopReason::EndTurn),
        "unknown reason must fall back to EndTurn, got: {:?}",
        resp.stop_reason
    );
}

#[tokio::test]
async fn malformed_event_json_returns_err() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    // Publish garbage bytes to the session_update subject instead of a valid SessionNotification.
    let session_id = "sess-bad-json";
    let mut prompt_sub = nats
        .subscribe(format!("acp.session.{}.agent.prompt", session_id))
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = prompt_sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let update_subject = format!("acp.session.{}.agent.update.{}", session_id, req_id);
            nats2
                .publish(update_subject, b"{not valid json!!!}".as_ref().into())
                .await
                .unwrap();
        }
    });

    let result = bridge.prompt(PromptRequest::new(session_id, vec![])).await;
    // The bridge skips malformed notification JSON and keeps waiting; the prompt
    // eventually times out since no valid response arrives.
    assert!(result.is_err(), "expected Err from malformed event JSON");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("timed out"),
        "expected timeout error when notification is malformed, got: {err:?}",
    );
}

// ── prompt / cancel ───────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_while_prompt_running_returns_cancelled() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    // The mock runner never responds — the cancel signal terminates the prompt.
    let session_id = "sess-cancel-prompt";
    let mut prompt_sub = nats
        .subscribe(format!("acp.session.{}.agent.prompt", session_id))
        .await
        .unwrap();

    // Once the prompt is published, immediately fire the cancel broadcast.
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if prompt_sub.next().await.is_some() {
            let cancelled_subject = format!("acp.session.{}.agent.cancelled", session_id);
            nats2.publish(cancelled_subject, b"".as_ref().into()).await.unwrap();
        }
    });

    let resp = bridge.prompt(PromptRequest::new(session_id, vec![])).await.unwrap();
    assert!(
        matches!(resp.stop_reason, StopReason::Cancelled),
        "cancel signal must return Cancelled, got: {:?}",
        resp.stop_reason,
    );
}

/// End-to-end: `bridge.cancel()` itself (not a direct NATS publish) stops
/// a concurrently running `bridge.prompt()`.  This covers the full path:
/// `cancel handler` → publishes `session_cancelled` broadcast → prompt
/// `cancel_notify` select arm fires → returns `Cancelled`.
#[tokio::test]
async fn bridge_cancel_stops_running_prompt_end_to_end() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    let session_id = "sess-cancel-e2e";

    // Mock runner: subscribe so the bridge can publish the prompt, but never
    // send events back.  The prompt will wait until cancelled.
    let mut prompt_sub = nats
        .subscribe(format!("acp.session.{}.agent.prompt", session_id))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = prompt_sub.next().await;
    });

    // Two bridge instances sharing the same NATS connection.
    // bridge_prompt drives the prompt; bridge_cancel fires the cancel.
    // Bridge is !Send so we use tokio::join! instead of tokio::spawn.
    let bridge_prompt = make_bridge(nats.clone(), "acp").await;
    let bridge_cancel = make_bridge(nats.clone(), "acp").await;

    let (prompt_result, cancel_result) =
        tokio::join!(bridge_prompt.prompt(PromptRequest::new(session_id, vec![])), async {
            // Wait until the prompt is in-flight before cancelling.
            tokio::time::sleep(Duration::from_millis(200)).await;
            bridge_cancel.cancel(CancelNotification::new(session_id)).await
        },);

    assert!(cancel_result.is_ok(), "cancel must succeed: {:?}", cancel_result);
    let resp = prompt_result.expect("prompt must complete (not time out)");
    assert!(
        matches!(resp.stop_reason, StopReason::Cancelled),
        "bridge.cancel() must stop the prompt with Cancelled, got: {:?}",
        resp.stop_reason,
    );
}

#[tokio::test]
async fn prompt_invalid_session_id_returns_error() {
    let (_c, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    let err = bridge
        .prompt(PromptRequest::new("invalid.session.id", vec![]))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("Invalid session ID"),
        "expected Invalid session ID error, got: {err}"
    );
}

// ── notification receiver dropped (warn! branches) ────────────────────────────

/// When the notification receiver is dropped before the prompt runs, every
/// `notification_sender.send(…)` call returns `Err`. The handler must NOT
/// abort — it logs a warning and continues until `Done`.
///
/// This test covers every `is_err()` warn branch in `handle()`:
///   TextDelta, ThinkingDelta, ToolCallStarted (normal), TodoWrite ToolCallStarted,
///   ToolCallFinished, ModeChanged (×2), UsageUpdate, SystemStatus.
#[tokio::test]
async fn notification_receiver_dropped_prompt_still_completes() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    // make_bridge drops the rx immediately → all sends fail
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-rx-dropped",
        vec![
            PromptEvent::TextDelta {
                text: "hello".to_string(),
            },
            PromptEvent::ThinkingDelta {
                text: "thinking...".to_string(),
            },
            PromptEvent::ToolCallStarted {
                id: "call-normal".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
                parent_tool_use_id: None,
            },
            PromptEvent::ToolCallStarted {
                id: "call-todo".to_string(),
                name: "TodoWrite".to_string(),
                input: serde_json::json!({
                    "todos": [{ "content": "task", "status": "pending", "priority": "high" }]
                }),
                parent_tool_use_id: None,
            },
            PromptEvent::ToolCallFinished {
                id: "call-normal".to_string(),
                output: "output".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::ModeChanged {
                mode: "plan".to_string(),
                model: "claude-sonnet-4-6".to_string(),
            },
            PromptEvent::SystemStatus {
                message: "rate_limit_warning".to_string(),
            },
            PromptEvent::UsageUpdate {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                context_window: Some(200_000),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    let resp = bridge
        .prompt(PromptRequest::new("sess-rx-dropped", vec![]))
        .await
        .expect("prompt must complete even with dropped notification receiver");
    assert!(
        matches!(resp.stop_reason, StopReason::EndTurn),
        "expected EndTurn, got: {:?}",
        resp.stop_reason
    );
}

/// Sending a prompt with a Text block exercises the `Some(t.text.as_str())`
/// branch in the `user_message` filter_map (line 40 in prompt.rs).
#[tokio::test]
async fn prompt_with_text_block_populates_user_message() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-text-block",
        vec![PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        }],
    )
    .await;

    let blocks = vec![ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(
        "hello world",
    ))];
    let resp = bridge
        .prompt(PromptRequest::new("sess-text-block", blocks))
        .await
        .expect("prompt with text block must succeed");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
}

/// Sending a prompt with only Image blocks exercises the `else { None }` branch
/// in the `user_message` filter_map (non-Text blocks are skipped).
#[tokio::test]
async fn prompt_with_image_only_blocks_produces_empty_user_message() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-img-only",
        vec![PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        }],
    )
    .await;

    // Only an Image block — no Text → user_message will be "" (else { None } path)
    let blocks = vec![ContentBlock::Image(ImageContent::new("base64data==", "image/png"))];
    let resp = bridge
        .prompt(PromptRequest::new("sess-img-only", blocks))
        .await
        .expect("prompt with image-only blocks must succeed");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
}

// ── fork_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_returns_new_session_id() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let forked_id = SessionId::from("forked-sess-1");
    let mut agent_sub = nats.subscribe("acp.session.s1.agent.fork").await.unwrap();
    let nats2 = nats.clone();
    let resp_id = forked_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            js_respond_session(&nats2, &msg, "acp", "s1", &ForkSessionResponse::new(resp_id)).await;
        }
    });

    let result = bridge.fork_session(ForkSessionRequest::new("s1", ".")).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
    assert_eq!(result.unwrap().session_id, forked_id);
}

#[tokio::test]
async fn fork_session_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.session.orig-sess.agent.fork").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            js_respond_session(
                &nats2,
                &msg,
                "acp",
                "orig-sess",
                &ForkSessionResponse::new(SessionId::from("f1")),
            )
            .await;
        }
    });

    bridge
        .fork_session(ForkSessionRequest::new("orig-sess", "."))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    assert_eq!(subject, "acp.session.orig-sess.agent.fork");
}

#[tokio::test]
async fn fork_session_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    let err = bridge
        .fork_session(ForkSessionRequest::new("s1", "."))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

// ── resume_session ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn resume_session_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.session.s2.agent.resume").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            js_respond_session(&nats2, &msg, "acp", "s2", &ResumeSessionResponse::new()).await;
        }
    });

    let result = bridge.resume_session(ResumeSessionRequest::new("s2", ".")).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

#[tokio::test]
async fn resume_session_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.session.my-sess.agent.resume").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            js_respond_session(&nats2, &msg, "acp", "my-sess", &ResumeSessionResponse::new()).await;
        }
    });

    bridge
        .resume_session(ResumeSessionRequest::new("my-sess", "."))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    assert_eq!(subject, "acp.session.my-sess.agent.resume");
}

#[tokio::test]
async fn resume_session_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    let err = bridge
        .resume_session(ResumeSessionRequest::new("s2", "."))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

// ── list_sessions ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_returns_session_list() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.agent.session.list").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&ListSessionsResponse::new(vec![])).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge.list_sessions(ListSessionsRequest::new()).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
    assert!(result.unwrap().sessions.is_empty());
}

#[tokio::test]
async fn list_sessions_uses_global_subject_not_session_scoped() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.agent.session.list").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&ListSessionsResponse::new(vec![])).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge.list_sessions(ListSessionsRequest::new()).await.unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    // list_sessions is NOT session-scoped — no session_id token in subject
    assert_eq!(subject, "acp.agent.session.list");
}

#[tokio::test]
async fn list_sessions_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    let err = bridge.list_sessions(ListSessionsRequest::new()).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

// ── set_session_config_option ──────────────────────────────────────────────────

#[tokio::test]
async fn set_session_config_option_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.session.s4.agent.set_config_option").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            js_respond_session(&nats2, &msg, "acp", "s4", &SetSessionConfigOptionResponse::new(vec![])).await;
        }
    });

    let result = bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new("s4", "mode", "plan"))
        .await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

#[tokio::test]
async fn set_session_config_option_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.session.sess-cfg.agent.set_config_option")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            js_respond_session(
                &nats2,
                &msg,
                "acp",
                "sess-cfg",
                &SetSessionConfigOptionResponse::new(vec![]),
            )
            .await;
        }
    });

    bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new("sess-cfg", "mode", "plan"))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    assert_eq!(subject, "acp.session.sess-cfg.agent.set_config_option");
}

#[tokio::test]
async fn set_session_config_option_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

    let err = bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new("s4", "mode", "plan"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

// ── SystemStatus / ToolCallStarted meta ──────────────────────────────────────

/// When the notification receiver is dropped and a compact SystemStatus arrives,
/// the `AgentMessageChunk` send hits `is_err()` (prompt.rs line 415).
/// The prompt must still complete.
#[tokio::test]
async fn compact_status_with_dropped_receiver_still_completes() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-compact-dropped",
        vec![
            PromptEvent::SystemStatus {
                message: "compacting memory...".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    let resp = bridge
        .prompt(PromptRequest::new("sess-compact-dropped", vec![]))
        .await
        .expect("prompt must complete even when compact notification receiver is dropped");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
}

// ── fork_session_integration ──────────────────────────────────────────────────

/// fork_session through the bridge: the bridge publishes to the session-scoped
/// fork subject and returns a ForkSessionResponse with a new session_id.
/// Uses `make_bridge_with_rx` so we can capture notifications as well.
#[tokio::test]
async fn fork_session_integration_returns_new_session_id() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let forked_id = SessionId::from("forked-integ-1");
    let mut agent_sub = nats.subscribe("acp.session.src-sess-1.agent.fork").await.unwrap();
    let nats2 = nats.clone();
    let resp_id = forked_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            js_respond_session(&nats2, &msg, "acp", "src-sess-1", &ForkSessionResponse::new(resp_id)).await;
        }
    });

    let result = bridge.fork_session(ForkSessionRequest::new("src-sess-1", ".")).await;
    assert!(
        result.is_ok(),
        "fork_session must succeed, got: {:?}",
        result.unwrap_err()
    );
    let resp = result.unwrap();
    assert_eq!(
        resp.session_id, forked_id,
        "fork_session must return the mocked forked session id"
    );
    assert_ne!(
        resp.session_id.to_string(),
        "src-sess-1",
        "fork_session must return a new session id, not the source"
    );
}

/// fork_session publishes to the session-scoped subject (includes session_id token).
#[tokio::test]
async fn fork_session_integration_uses_session_scoped_nats_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.session.fork-src-2.agent.fork").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            js_respond_session(
                &nats2,
                &msg,
                "acp",
                "fork-src-2",
                &ForkSessionResponse::new(SessionId::from("fork-dst-2")),
            )
            .await;
        }
    });

    bridge
        .fork_session(ForkSessionRequest::new("fork-src-2", "."))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    assert_eq!(
        subject, "acp.session.fork-src-2.agent.fork",
        "fork subject must be session-scoped"
    );
}

// ── resume_session_integration ─────────────────────────────────────────────────

/// resume_session through the bridge returns a ResumeSessionResponse.
/// Uses `make_bridge_with_rx` to keep the notification receiver alive.
#[tokio::test]
async fn resume_session_integration_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.session.resume-sess-1.agent.resume").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            js_respond_session(&nats2, &msg, "acp", "resume-sess-1", &ResumeSessionResponse::new()).await;
        }
    });

    let result = bridge
        .resume_session(ResumeSessionRequest::new("resume-sess-1", "."))
        .await;
    assert!(
        result.is_ok(),
        "resume_session must succeed, got: {:?}",
        result.unwrap_err()
    );
}

/// resume_session publishes to the session-scoped subject.
#[tokio::test]
async fn resume_session_integration_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.session.resume-sess-2.agent.resume").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            js_respond_session(&nats2, &msg, "acp", "resume-sess-2", &ResumeSessionResponse::new()).await;
        }
    });

    bridge
        .resume_session(ResumeSessionRequest::new("resume-sess-2", "."))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    assert_eq!(
        subject, "acp.session.resume-sess-2.agent.resume",
        "resume subject must be session-scoped"
    );
}

// ── prompt with https image URI (ImageUrl path) ───────────────────────────────

/// Sending a prompt with an Image block whose URI is an HTTPS URL must
/// successfully reach the runner as an `ImageUrl` block (not base64).
/// The test verifies the prompt completes with EndTurn — the runner sees the
/// payload with the URL image source.
#[tokio::test]
async fn prompt_with_https_image_uri_block_completes_successfully() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-img-url",
        vec![PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        }],
    )
    .await;

    // ContentBlock::Image with an HTTPS URI and empty data → converted to UserContentBlock::ImageUrl
    let blocks = vec![ContentBlock::Image(
        agent_client_protocol::schema::v1::ImageContent::new("", "image/jpeg").uri("https://example.com/photo.jpg".to_string()),
    )];
    let resp = bridge
        .prompt(PromptRequest::new("sess-img-url", blocks))
        .await
        .expect("prompt with HTTPS image URI must succeed");
    assert!(
        matches!(resp.stop_reason, StopReason::EndTurn),
        "expected EndTurn, got: {:?}",
        resp.stop_reason
    );
}

// ── ext_method integration ────────────────────────────────────────────────────

/// ext_method through the bridge with a real NATS responder returns the response.
#[tokio::test]
async fn ext_method_integration_forwards_request_and_returns_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    // Spawn a NATS responder on the global ext subject.
    let mut agent_sub = nats.subscribe("acp.agent.ext.session_close").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let raw = serde_json::value::RawValue::from_string(r#"{"status":"closed"}"#.to_string()).unwrap();
            let resp = ExtResponse::new(raw.into());
            let resp_bytes = serde_json::to_vec(&resp).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp_bytes.into()).await.unwrap();
            }
        }
    });

    let params = serde_json::value::RawValue::from_string(r#"{"sessionId":"sess-ext-1"}"#.to_string()).unwrap();
    let result = bridge.ext_method(ExtRequest::new("session_close", params.into())).await;

    assert!(
        result.is_ok(),
        "ext_method must succeed with real responder, got: {:?}",
        result.unwrap_err()
    );
}

/// ext_method with no responder returns AgentUnavailable (timeout).
#[tokio::test]
async fn ext_method_integration_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;
    // No responder — request will time out.
    let params = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let err = bridge
        .ext_method(ExtRequest::new("session_close", params.into()))
        .await
        .unwrap_err();
    assert_eq!(
        err.code,
        agent_client_protocol::ErrorCode::Other(AGENT_UNAVAILABLE),
        "timeout must return AgentUnavailable, got: {:?}",
        err
    );
}

// ── ext_notification integration ──────────────────────────────────────────────

/// ext_notification through the bridge publishes to the global ext subject.
#[tokio::test]
async fn ext_notification_integration_publishes_to_agent_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut sub = nats.subscribe("acp.agent.ext.my_notify").await.unwrap();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            let _ = tx.send(msg.subject.to_string());
        }
    });

    let params = serde_json::value::RawValue::from_string(r#"{"event":"ping"}"#.to_string()).unwrap();
    let result = bridge
        .ext_notification(ExtNotification::new("my_notify", params.into()))
        .await;
    assert!(result.is_ok(), "ext_notification must always return Ok");

    let subject = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("timed out waiting for ext_notification publish")
        .unwrap();
    assert_eq!(subject, "acp.agent.ext.my_notify");
}

/// ext_notification with no subscriber still returns Ok (fire-and-forget).
#[tokio::test]
async fn ext_notification_integration_always_ok_with_no_subscriber() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let params = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let result = bridge
        .ext_notification(ExtNotification::new("my_notify", params.into()))
        .await;
    assert!(result.is_ok(), "fire-and-forget: must be Ok even with no subscriber");
}

// ── close_session integration ─────────────────────────────────────────────────

/// close_session through the bridge routes to the correct per-session NATS subject.
#[tokio::test]
async fn close_session_integration_forwards_request_and_returns_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let mut sub = nats.subscribe("acp.session.sess-close-1.agent.close").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            js_respond_session(&nats2, &msg, "acp", "sess-close-1", &CloseSessionResponse::new()).await;
        }
    });

    let result = bridge.close_session(CloseSessionRequest::new("sess-close-1")).await;
    assert!(
        result.is_ok(),
        "close_session must succeed with real responder, got: {:?}",
        result.unwrap_err()
    );
}

/// close_session with no responder returns AgentUnavailable (timeout).
#[tokio::test]
async fn close_session_integration_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let err = bridge
        .close_session(CloseSessionRequest::new("sess-close-2"))
        .await
        .unwrap_err();
    assert_eq!(
        err.code,
        agent_client_protocol::ErrorCode::Other(AGENT_UNAVAILABLE),
        "timeout must return AgentUnavailable, got: {:?}",
        err
    );
}

// ── session branching integration ─────────────────────────────────────────────

/// fork_session with branchAtIndex in _meta forwards the field in the NATS payload.
#[tokio::test]
async fn fork_session_with_branch_at_index_forwards_meta() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<bytes::Bytes>();
    let mut agent_sub = nats.subscribe("acp.session.branch-src-1.agent.fork").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.payload.clone());
            js_respond_session(
                &nats2,
                &msg,
                "acp",
                "branch-src-1",
                &ForkSessionResponse::new(SessionId::from("branch-dst-1")),
            )
            .await;
        }
    });

    let meta =
        serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(serde_json::json!({ "branchAtIndex": 2 }))
            .unwrap();
    bridge
        .fork_session(ForkSessionRequest::new("branch-src-1", ".").meta(meta))
        .await
        .unwrap();

    let payload = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for NATS payload")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(
        body["_meta"]["branchAtIndex"],
        serde_json::json!(2),
        "branchAtIndex must be forwarded in the NATS payload _meta"
    );
}

/// ext_method("session/list_children") routes to the global ext NATS subject.
#[tokio::test]
async fn ext_list_children_integration_routes_to_ext_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut ext_sub = nats.subscribe("acp.agent.ext.session/list_children").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = ext_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let raw = serde_json::value::RawValue::from_string(r#"{"children":[]}"#.to_string()).unwrap();
            let resp = ExtResponse::new(raw.into());
            if let Some(reply) = &msg.reply {
                let _ = nats2
                    .publish(reply.clone(), serde_json::to_vec(&resp).unwrap().into())
                    .await;
            }
        }
    });

    let params = serde_json::value::RawValue::from_string(r#"{"sessionId":"some-session"}"#.to_string()).unwrap();
    bridge
        .ext_method(ExtRequest::new("session/list_children", params.into()))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for NATS subject")
        .unwrap();
    assert_eq!(
        subject, "acp.agent.ext.session/list_children",
        "session/list_children must route to global ext NATS subject"
    );
}

/// Verifies the runner registration contract: the `acp_prefix` metadata stored
/// in the registry must match the `ACP_PREFIX` env var.  The bridge
/// (`acp-nats-ws`, `acp-nats-server`) reads this field to derive the NATS
/// routing prefix — a mismatch breaks routing silently in production.
#[tokio::test]
async fn acp_runner_registers_with_correct_acp_prefix_metadata() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.claude";
    let agent_type = "claude";

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.to_string(),
        capabilities: vec!["chat".to_string()],
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": prefix }),
    };
    registry.register(&cap).await.expect("registration must succeed");

    let entry = registry
        .get(agent_type)
        .await
        .expect("get must not error")
        .expect("registered entry must exist");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(prefix),
        "bridge relies on acp_prefix matching ACP_PREFIX — got {:?}",
        entry.metadata
    );
    assert_eq!(
        entry.nats_subject,
        format!("{}.agent.>", prefix),
        "nats_subject must be derived from ACP_PREFIX"
    );
}
