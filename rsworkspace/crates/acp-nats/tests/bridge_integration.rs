//! Integration tests for acp-nats Bridge with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p acp-nats --test bridge_integration

use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

use acp_nats::prompt_event::PromptEvent;
use acp_nats::{AGENT_UNAVAILABLE, AcpPrefix, Bridge, Config, NatsAuth, NatsConfig};
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, ClientCapabilities,
    ContentBlock, ErrorCode, ForkSessionRequest, ForkSessionResponse, ImageContent, Implementation,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason,
    ToolCallContent, ToolCallStatus, ToolKind,
};
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_std::time::SystemClock;

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
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

fn make_bridge(nats: async_nats::Client, prefix: &str) -> Bridge<async_nats::Client, SystemClock> {
    let config = Config::new(
        AcpPrefix::new(prefix).unwrap(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500));
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Bridge::new(
        nats,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-integration-test"),
        config,
        tx,
    )
}

/// Like `make_bridge` but keeps the notification receiver alive so tests can
/// assert on the `SessionNotification`s produced during a prompt.
fn make_bridge_with_rx(
    nats: async_nats::Client,
    prefix: &str,
) -> (
    Bridge<async_nats::Client, SystemClock>,
    tokio::sync::mpsc::Receiver<agent_client_protocol::SessionNotification>,
) {
    let config = Config::new(
        AcpPrefix::new(prefix).unwrap(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500));
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let bridge = Bridge::new(
        nats,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-integration-test"),
        config,
        tx,
    );
    (bridge, rx)
}

// ── initialize ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_returns_protocol_version_from_agent() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await;

    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
    assert_eq!(result.unwrap().protocol_version, ProtocolVersion::LATEST);
}

#[tokio::test]
async fn initialize_returns_agent_unavailable_when_no_agent() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp");

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
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await
            && let Some(reply) = msg.reply
        {
            // Send malformed JSON.
            nats2
                .publish(reply, b"{bad json}".as_ref().into())
                .await
                .unwrap();
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
    let bridge = make_bridge(nats.clone(), "acp");

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

    let result = bridge
        .authenticate(AuthenticateRequest::new("password"))
        .await;
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
}

#[tokio::test]
async fn authenticate_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp");

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
    let bridge = make_bridge(nats.clone(), "acp");

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
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
    assert_eq!(result.unwrap().session_id, expected_id);
}

#[tokio::test]
async fn new_session_publishes_session_ready_after_delay() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let session_id = SessionId::from("sess-ready-test");

    // Subscribe to the session.ready notification BEFORE calling new_session.
    let ready_subject = format!("acp.{}.agent.ext.session.ready", session_id);
    let mut ready_sub = nats.subscribe(ready_subject.clone()).await.unwrap();

    // Set up mock agent.
    let mut agent_sub = nats.subscribe("acp.agent.session.new").await.unwrap();
    let nats2 = nats.clone();
    let resp_id = session_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&NewSessionResponse::new(resp_id)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .new_session(NewSessionRequest::new("."))
        .await
        .unwrap();

    // The session.ready is published ~100ms after new_session returns.
    let msg = tokio::time::timeout(Duration::from_secs(2), ready_sub.next())
        .await
        .expect("timed out waiting for session.ready")
        .expect("subscriber closed");

    assert_eq!(msg.subject.as_str(), ready_subject.as_str());
}

// ── load_session ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_session_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.s1.agent.session.load").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let subject = msg.subject.to_string();
            let _ = tx.send(subject);
            // Respond so bridge doesn't time out.
            let resp = serde_json::to_vec(&LoadSessionResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .load_session(LoadSessionRequest::new("s1", "."))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();

    assert_eq!(subject, "acp.s1.agent.session.load");
}

#[tokio::test]
async fn load_session_invalid_session_id_returns_error_without_nats() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

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

#[tokio::test]
async fn load_session_publishes_session_ready() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let session_id = "ls-ready-test";
    let ready_subject = format!("acp.{}.agent.ext.session.ready", session_id);
    let mut ready_sub = nats.subscribe(ready_subject.clone()).await.unwrap();

    let mut agent_sub = nats
        .subscribe(format!("acp.{}.agent.session.load", session_id))
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&LoadSessionResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .load_session(LoadSessionRequest::new(session_id, "."))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), ready_sub.next())
        .await
        .expect("timed out waiting for session.ready")
        .expect("subscriber closed");

    assert_eq!(msg.subject.as_str(), ready_subject.as_str());
}

// ── set_session_mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_mode_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats
        .subscribe("acp.s1.agent.session.set_mode")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&SetSessionModeResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge
        .set_session_mode(SetSessionModeRequest::new("s1", "edit"))
        .await;
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
}

// ── cancel ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_publishes_to_correct_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut sub = nats.subscribe("acp.s1.agent.session.cancel").await.unwrap();

    bridge.cancel(CancelNotification::new("s1")).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(2), sub.next())
        .await
        .expect("timed out waiting for cancel message")
        .expect("subscriber closed");

    assert_eq!(msg.subject.as_str(), "acp.s1.agent.session.cancel");
}

#[tokio::test]
async fn cancel_always_returns_ok_even_if_no_subscriber() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp");

    // Fire-and-forget: no subscriber, but cancel still returns Ok(()).
    let result = bridge.cancel(CancelNotification::new("s1")).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn cancel_invalid_session_id_returns_error_before_publish() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

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
    let bridge = make_bridge(nats.clone(), "custom");

    // The subject must start with "custom.", not "acp.".
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("custom.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let subject = msg.subject.to_string();
            let _ = tx.send(subject);
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
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
    let bridge = make_bridge(nats.clone(), "acp");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            // Capture the raw payload to verify the client name is present.
            let payload_str = String::from_utf8_lossy(&msg.payload).to_string();
            let _ = tx.send(payload_str);
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .initialize(
            InitializeRequest::new(ProtocolVersion::LATEST)
                .client_info(Implementation::new("my-client", "1.0.0")),
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
    let bridge = make_bridge(nats.clone(), "acp");

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
        assert!(
            result.is_ok(),
            "concurrent request failed: {:?}",
            result.unwrap_err()
        );
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

/// Spawn a mock runner that subscribes to `{prefix}.{session_id}.agent.prompt`,
/// extracts `req_id` from the payload, and publishes the given `events` back on
/// `{prefix}.{session_id}.agent.prompt.events.{req_id}`.
///
/// Returns only after the NATS subscription is confirmed, eliminating the race
/// where the bridge publishes the prompt before the mock has subscribed.
async fn mock_runner(
    nats: async_nats::Client,
    prefix: &str,
    session_id: &str,
    events: Vec<PromptEvent>,
) {
    let subject = format!("{}.{}.agent.prompt", prefix, session_id);
    let prefix = prefix.to_string();
    let session_id = session_id.to_string();
    // Subscribe BEFORE returning so the bridge can't miss the prompt.
    let mut sub = nats.subscribe(subject).await.unwrap();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            let payload: acp_nats::prompt_event::PromptPayload =
                serde_json::from_slice(&msg.payload).expect("valid PromptPayload");
            let events_subject = format!(
                "{}.{}.agent.prompt.events.{}",
                prefix, session_id, payload.req_id
            );
            for event in events {
                nats.publish(
                    events_subject.clone(),
                    serde_json::to_vec(&event).unwrap().into(),
                )
                .await
                .unwrap();
            }
        }
    });
}

// ── prompt / ModeChanged ──────────────────────────────────────────────────────

/// Helper: drain all notifications from `rx` into a `Vec<SessionUpdate>`.
fn drain_updates(
    rx: &mut tokio::sync::mpsc::Receiver<agent_client_protocol::SessionNotification>,
) -> Vec<SessionUpdate> {
    let mut v = Vec::new();
    while let Ok(n) = rx.try_recv() {
        v.push(n.update);
    }
    v
}

#[tokio::test]
async fn mode_changed_event_produces_current_mode_and_config_option_notifications() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-mode-test",
        vec![
            PromptEvent::ModeChanged {
                mode: "plan".to_string(),
                model: "claude-opus-4-6".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-mode-test", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let mode_pos = updates
        .iter()
        .position(|u| matches!(u, SessionUpdate::CurrentModeUpdate(_)));
    let cfg_pos = updates
        .iter()
        .position(|u| matches!(u, SessionUpdate::ConfigOptionUpdate(_)));
    assert!(mode_pos.is_some(), "expected CurrentModeUpdate");
    assert!(cfg_pos.is_some(), "expected ConfigOptionUpdate");
    assert!(
        mode_pos.unwrap() < cfg_pos.unwrap(),
        "CurrentModeUpdate must precede ConfigOptionUpdate"
    );
}

#[tokio::test]
async fn mode_changed_current_mode_update_carries_plan_mode() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-mode-value",
        vec![
            PromptEvent::ModeChanged {
                mode: "plan".to_string(),
                model: "claude-sonnet-4-6".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-mode-value", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let m = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::CurrentModeUpdate(m) = u {
                Some(m)
            } else {
                None
            }
        })
        .expect("must have CurrentModeUpdate");
    assert_eq!(m.current_mode_id.0.as_ref(), "plan");
}

#[tokio::test]
async fn no_mode_notifications_when_runner_does_not_emit_mode_changed() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-no-mode",
        vec![PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        }],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-no-mode", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, SessionUpdate::CurrentModeUpdate(_)))
    );
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, SessionUpdate::ConfigOptionUpdate(_)))
    );
}

// ── prompt / event types ──────────────────────────────────────────────────────

#[tokio::test]
async fn text_delta_produces_agent_message_chunk_notification() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-text",
        vec![
            PromptEvent::TextDelta {
                text: "hello world".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-text", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let chunk = updates.iter().find_map(|u| {
        if let SessionUpdate::AgentMessageChunk(c) = u {
            Some(c)
        } else {
            None
        }
    });
    assert!(
        chunk.is_some(),
        "expected AgentMessageChunk, got: {updates:?}"
    );
}

#[tokio::test]
async fn thinking_delta_produces_agent_thought_chunk_notification() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-think",
        vec![
            PromptEvent::ThinkingDelta {
                text: "reasoning...".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-think", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let chunk = updates.iter().find_map(|u| {
        if let SessionUpdate::AgentThoughtChunk(c) = u {
            Some(c)
        } else {
            None
        }
    });
    assert!(
        chunk.is_some(),
        "expected AgentThoughtChunk, got: {updates:?}"
    );
}

#[tokio::test]
async fn error_event_returns_err_from_prompt() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

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
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("something blew up")
    );
}

#[tokio::test]
async fn usage_update_produces_usage_notification() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-usage",
        vec![
            PromptEvent::UsageUpdate {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 10,
                cache_read_tokens: 5,
                context_window: Some(200_000),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-usage", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, SessionUpdate::UsageUpdate(_))),
        "expected UsageUpdate notification, got: {updates:?}",
    );
}

#[tokio::test]
async fn done_stop_reason_end_turn_maps_correctly() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-done-et",
        vec![PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        }],
    )
    .await;

    let resp = bridge
        .prompt(PromptRequest::new("sess-done-et", vec![]))
        .await
        .unwrap();
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
    let bridge = make_bridge(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-done-mt",
        vec![PromptEvent::Done {
            stop_reason: "max_tokens".to_string(),
        }],
    )
    .await;

    let resp = bridge
        .prompt(PromptRequest::new("sess-done-mt", vec![]))
        .await
        .unwrap();
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
    let bridge = make_bridge(nats.clone(), "acp");

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
    let bridge = make_bridge(nats.clone(), "acp");

    // Publish garbage bytes instead of a valid PromptEvent JSON.
    let session_id = "sess-bad-json";
    let mut prompt_sub = nats
        .subscribe(format!("acp.{}.agent.prompt", session_id))
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = prompt_sub.next().await {
            let payload: acp_nats::prompt_event::PromptPayload =
                serde_json::from_slice(&msg.payload).unwrap();
            let events_subject =
                format!("acp.{}.agent.prompt.events.{}", session_id, payload.req_id);
            nats2
                .publish(events_subject, b"{not valid json!!!}".as_ref().into())
                .await
                .unwrap();
        }
    });

    let result = bridge.prompt(PromptRequest::new(session_id, vec![])).await;
    assert!(result.is_err(), "expected Err from malformed event JSON");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("bad event payload"),
        "error message should mention bad event payload",
    );
}

// ── prompt / tool calls ───────────────────────────────────────────────────────

#[tokio::test]
async fn tool_call_started_produces_tool_call_in_progress_notification() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-tc-start",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-1".to_string(),
                name: "get_pr_diff".to_string(),
                input: serde_json::json!({"owner": "acme", "repo": "api", "pr_number": 42}),
            },
            PromptEvent::ToolCallFinished {
                id: "call-1".to_string(),
                output: "diff output".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-tc-start", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let tool_call = updates.iter().find_map(|u| {
        if let SessionUpdate::ToolCall(tc) = u {
            Some(tc)
        } else {
            None
        }
    });
    assert!(
        tool_call.is_some(),
        "expected ToolCall notification, got: {updates:?}"
    );
    let tc = tool_call.unwrap();
    assert!(matches!(tc.status, ToolCallStatus::InProgress));
}

#[tokio::test]
async fn tool_call_finished_success_produces_completed_update() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-tc-ok",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-ok".to_string(),
                name: "list_pr_files".to_string(),
                input: serde_json::json!({}),
            },
            PromptEvent::ToolCallFinished {
                id: "call-ok".to_string(),
                output: "file list".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-tc-ok", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let update = updates.iter().find_map(|u| {
        if let SessionUpdate::ToolCallUpdate(u) = u {
            Some(u)
        } else {
            None
        }
    });
    assert!(
        update.is_some(),
        "expected ToolCallUpdate, got: {updates:?}"
    );
    let status = update.unwrap().fields.status;
    assert!(
        matches!(status, Some(ToolCallStatus::Completed)),
        "got: {status:?}"
    );
}

#[tokio::test]
async fn tool_call_finished_nonzero_exit_code_produces_failed_update() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-tc-fail",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-fail".to_string(),
                name: "update_file".to_string(),
                input: serde_json::json!({}),
            },
            PromptEvent::ToolCallFinished {
                id: "call-fail".to_string(),
                output: "permission denied".to_string(),
                exit_code: Some(1),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-tc-fail", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let update = updates.iter().find_map(|u| {
        if let SessionUpdate::ToolCallUpdate(u) = u {
            Some(u)
        } else {
            None
        }
    });
    assert!(
        update.is_some(),
        "expected ToolCallUpdate for failed tool, got: {updates:?}"
    );
    let status = update.unwrap().fields.status;
    assert!(
        matches!(status, Some(ToolCallStatus::Failed)),
        "non-zero exit must map to Failed, got: {status:?}"
    );
}

#[tokio::test]
async fn tool_call_finished_with_signal_produces_failed_update() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-tc-sig",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-sig".to_string(),
                name: "post_pr_comment".to_string(),
                input: serde_json::json!({}),
            },
            PromptEvent::ToolCallFinished {
                id: "call-sig".to_string(),
                output: "killed".to_string(),
                exit_code: None,
                signal: Some("SIGTERM".to_string()),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-tc-sig", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let update = updates.iter().find_map(|u| {
        if let SessionUpdate::ToolCallUpdate(u) = u {
            Some(u)
        } else {
            None
        }
    });
    assert!(
        update.is_some(),
        "expected ToolCallUpdate for signalled tool"
    );
    let status = update.unwrap().fields.status;
    assert!(
        matches!(status, Some(ToolCallStatus::Failed)),
        "signal must map to Failed, got: {status:?}"
    );
}

#[tokio::test]
async fn duplicate_tool_id_is_silently_ignored() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    // Same tool id sent twice — the second must be a no-op (not double-counted).
    mock_runner(
        nats,
        "acp",
        "sess-tc-dup",
        vec![
            PromptEvent::ToolCallStarted {
                id: "dup-id".to_string(),
                name: "get_pr_diff".to_string(),
                input: serde_json::json!({}),
            },
            PromptEvent::ToolCallStarted {
                id: "dup-id".to_string(),
                name: "get_pr_diff".to_string(),
                input: serde_json::json!({}),
            },
            PromptEvent::ToolCallFinished {
                id: "dup-id".to_string(),
                output: "ok".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-tc-dup", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let tool_call_count = updates
        .iter()
        .filter(|u| matches!(u, SessionUpdate::ToolCall(_)))
        .count();
    assert_eq!(
        tool_call_count, 1,
        "duplicate ToolCallStarted must produce exactly one ToolCall notification"
    );
}

// ── prompt / TodoWrite ────────────────────────────────────────────────────────

#[tokio::test]
async fn todo_write_produces_plan_notification_not_tool_call() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-todo",
        vec![
            PromptEvent::ToolCallStarted {
                id: "todo-1".to_string(),
                name: "TodoWrite".to_string(),
                input: serde_json::json!({
                    "todos": [
                        {"content": "Fix the bug", "status": "in_progress", "priority": "high"},
                        {"content": "Write tests", "status": "pending", "priority": "medium"},
                    ]
                }),
            },
            // Finished event for TodoWrite must be silently skipped (no ToolCallUpdate).
            PromptEvent::ToolCallFinished {
                id: "todo-1".to_string(),
                output: "ok".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-todo", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);

    // Must have a Plan notification.
    let plan = updates.iter().find_map(|u| {
        if let SessionUpdate::Plan(p) = u {
            Some(p)
        } else {
            None
        }
    });
    assert!(
        plan.is_some(),
        "expected Plan notification for TodoWrite, got: {updates:?}"
    );

    // Must NOT have a ToolCall or ToolCallUpdate notification (TodoWrite is invisible).
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, SessionUpdate::ToolCall(_))),
        "TodoWrite must NOT produce a ToolCall notification",
    );
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, SessionUpdate::ToolCallUpdate(_))),
        "TodoWrite finish must NOT produce a ToolCallUpdate notification",
    );
}

// ── prompt / cancel ───────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_while_prompt_running_returns_cancelled() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    // The mock runner never responds — the cancel signal terminates the prompt.
    let session_id = "sess-cancel-prompt";
    let mut prompt_sub = nats
        .subscribe(format!("acp.{}.agent.prompt", session_id))
        .await
        .unwrap();

    // Once the prompt is published, immediately fire the cancel broadcast.
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if prompt_sub.next().await.is_some() {
            let cancelled_subject = format!("acp.{}.agent.session.cancelled", session_id);
            nats2
                .publish(cancelled_subject, b"".as_ref().into())
                .await
                .unwrap();
        }
    });

    let resp = bridge
        .prompt(PromptRequest::new(session_id, vec![]))
        .await
        .unwrap();
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
        .subscribe(format!("acp.{}.agent.prompt", session_id))
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = prompt_sub.next().await;
    });

    // Two bridge instances sharing the same NATS connection.
    // bridge_prompt drives the prompt; bridge_cancel fires the cancel.
    // Bridge is !Send so we use tokio::join! instead of tokio::spawn.
    let bridge_prompt = make_bridge(nats.clone(), "acp");
    let bridge_cancel = make_bridge(nats.clone(), "acp");

    let (prompt_result, cancel_result) = tokio::join!(
        bridge_prompt.prompt(PromptRequest::new(session_id, vec![])),
        async {
            // Wait until the prompt is in-flight before cancelling.
            tokio::time::sleep(Duration::from_millis(200)).await;
            bridge_cancel
                .cancel(CancelNotification::new(session_id))
                .await
        },
    );

    assert!(
        cancel_result.is_ok(),
        "cancel must succeed: {:?}",
        cancel_result
    );
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
    let bridge = make_bridge(nats, "acp");

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
    let bridge = make_bridge(nats.clone(), "acp");

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
            },
            PromptEvent::ToolCallStarted {
                id: "call-todo".to_string(),
                name: "TodoWrite".to_string(),
                input: serde_json::json!({
                    "todos": [{ "content": "task", "status": "pending", "priority": "high" }]
                }),
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
    let bridge = make_bridge(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-text-block",
        vec![PromptEvent::Done {
            stop_reason: "end_turn".to_string(),
        }],
    )
    .await;

    let blocks = vec![ContentBlock::Text(agent_client_protocol::TextContent::new(
        "hello world",
    ))];
    let resp = bridge
        .prompt(PromptRequest::new("sess-text-block", blocks))
        .await
        .expect("prompt with text block must succeed");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
}

/// When `ToolCallFinished` arrives for an ID that was never seen in
/// `ToolCallStarted`, the `tool_name_cache` lookup returns `None` and the
/// update is returned without meta (the `else update` branch, line 260).
#[tokio::test]
async fn tool_call_finished_without_prior_started_omits_meta() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-tc-no-start",
        vec![
            PromptEvent::ToolCallFinished {
                id: "unknown-call".to_string(),
                output: "output".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-tc-no-start", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let update = updates.iter().find_map(|u| {
        if let SessionUpdate::ToolCallUpdate(u) = u {
            Some(u)
        } else {
            None
        }
    });
    assert!(
        update.is_some(),
        "expected ToolCallUpdate for unknown id, got: {updates:?}"
    );
}

/// Sending a prompt with only Image blocks exercises the `else { None }` branch
/// in the `user_message` filter_map (non-Text blocks are skipped).
#[tokio::test]
async fn prompt_with_image_only_blocks_produces_empty_user_message() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

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
    let blocks = vec![ContentBlock::Image(ImageContent::new(
        "base64data==",
        "image/png",
    ))];
    let resp = bridge
        .prompt(PromptRequest::new("sess-img-only", blocks))
        .await
        .expect("prompt with image-only blocks must succeed");
    assert!(matches!(resp.stop_reason, StopReason::EndTurn));
}

// ── terminal streaming ─────────────────────────────────────────────────────────

/// When `terminal_output_cap` is set, a Bash tool call must produce three
/// notifications: ToolCall (InProgress + terminal_info meta), then two
/// ToolCallUpdate notifications (terminal_output, terminal_exit).
#[tokio::test]
async fn bash_with_terminal_output_cap_emits_three_notifications() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    // Enable terminal output capability directly — avoids needing a live
    // initialize round-trip while still exercising the same code path.
    bridge.set_terminal_output_cap(true);

    mock_runner(
        nats,
        "acp",
        "sess-term-bash",
        vec![
            PromptEvent::ToolCallStarted {
                id: "bash-term-1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({"command": "echo hello"}),
            },
            PromptEvent::ToolCallFinished {
                id: "bash-term-1".to_string(),
                output: "hello\n".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-term-bash", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);

    // 1. ToolCall with terminal_info in meta
    let tool_call = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::ToolCall(tc) = u {
                Some(tc)
            } else {
                None
            }
        })
        .expect("expected ToolCall notification");

    let meta = tool_call
        .meta
        .as_ref()
        .expect("Bash ToolCall must have meta when terminal_output_cap is set");
    assert!(
        meta.contains_key("terminal_info"),
        "ToolCall meta must contain terminal_info, got: {meta:?}"
    );
    let ti = meta["terminal_info"]
        .as_object()
        .expect("terminal_info must be an object");
    assert_eq!(
        ti.get("terminal_id").and_then(|v| v.as_str()),
        Some("bash-term-1"),
        "terminal_id must match tool call id"
    );

    // 2 & 3. Two ToolCallUpdate notifications: terminal_output then terminal_exit
    let update_notifications: Vec<_> = updates
        .iter()
        .filter_map(|u| {
            if let SessionUpdate::ToolCallUpdate(u) = u {
                Some(u)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        update_notifications.len(),
        2,
        "expected exactly 2 ToolCallUpdate notifications for terminal streaming, got: {update_notifications:?}"
    );

    // First: terminal_output
    let first = &update_notifications[0];
    let meta0 = first
        .meta
        .as_ref()
        .expect("terminal_output update must have meta");
    assert!(
        meta0.contains_key("terminal_output"),
        "first ToolCallUpdate meta must contain terminal_output, got: {meta0:?}"
    );
    let to = meta0["terminal_output"]
        .as_object()
        .expect("terminal_output must be an object");
    assert_eq!(
        to.get("data").and_then(|v| v.as_str()),
        Some("hello\n"),
        "terminal_output.data must equal the tool output"
    );

    // Second: terminal_exit with Completed status
    let second = &update_notifications[1];
    let meta1 = second
        .meta
        .as_ref()
        .expect("terminal_exit update must have meta");
    assert!(
        meta1.contains_key("terminal_exit"),
        "second ToolCallUpdate meta must contain terminal_exit, got: {meta1:?}"
    );
    assert!(
        matches!(second.fields.status, Some(ToolCallStatus::Completed)),
        "terminal_exit update must carry Completed status, got: {:?}",
        second.fields.status
    );
}

/// When `terminal_output_cap` is set and the Bash tool exits with a non-zero code,
/// the terminal_exit ToolCallUpdate must carry `Failed` status.
#[tokio::test]
async fn bash_with_terminal_output_cap_and_nonzero_exit_emits_failed_status() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");
    bridge.set_terminal_output_cap(true);

    mock_runner(
        nats,
        "acp",
        "sess-term-fail",
        vec![
            PromptEvent::ToolCallStarted {
                id: "bash-fail-1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({"command": "exit 1"}),
            },
            PromptEvent::ToolCallFinished {
                id: "bash-fail-1".to_string(),
                output: "error\n".to_string(),
                exit_code: Some(1),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-term-fail", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let exit_update = updates
        .iter()
        .filter_map(|u| {
            if let SessionUpdate::ToolCallUpdate(u) = u {
                if u.meta
                    .as_ref()
                    .is_some_and(|m| m.contains_key("terminal_exit"))
                {
                    Some(u)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .next()
        .expect("expected terminal_exit ToolCallUpdate");

    assert!(
        matches!(exit_update.fields.status, Some(ToolCallStatus::Failed)),
        "non-zero exit code must produce Failed status, got: {:?}",
        exit_update.fields.status
    );
}

/// Without terminal_output_cap, a Bash tool produces the normal 1 ToolCall + 1 ToolCallUpdate.
#[tokio::test]
async fn bash_without_terminal_output_cap_emits_standard_two_notifications() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");
    // terminal_output_cap defaults to false — no extra notifications

    mock_runner(
        nats,
        "acp",
        "sess-bash-std",
        vec![
            PromptEvent::ToolCallStarted {
                id: "bash-std-1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            },
            PromptEvent::ToolCallFinished {
                id: "bash-std-1".to_string(),
                output: "file.txt\n".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-bash-std", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);

    // Exactly 1 ToolCallUpdate (the standard completion path)
    let update_count = updates
        .iter()
        .filter(|u| matches!(u, SessionUpdate::ToolCallUpdate(_)))
        .count();
    assert_eq!(
        update_count, 1,
        "without terminal_output_cap, Bash must produce exactly 1 ToolCallUpdate"
    );

    // The ToolCall notification must NOT have terminal_info
    let tool_call = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::ToolCall(tc) = u {
                Some(tc)
            } else {
                None
            }
        })
        .expect("expected ToolCall notification");
    let has_terminal_info = tool_call
        .meta
        .as_ref()
        .is_some_and(|m| m.contains_key("terminal_info"));
    assert!(
        !has_terminal_info,
        "without cap, ToolCall must NOT have terminal_info in meta"
    );
}

// ── file locations ─────────────────────────────────────────────────────────────

/// Edit tool started notification must include the file path in locations and kind=Edit.
#[tokio::test]
async fn edit_tool_started_includes_file_location_and_kind() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-edit-loc",
        vec![
            PromptEvent::ToolCallStarted {
                id: "edit-loc-1".to_string(),
                name: "Edit".to_string(),
                input: serde_json::json!({
                    "file_path": "/src/main.rs",
                    "old_string": "foo",
                    "new_string": "bar"
                }),
            },
            PromptEvent::ToolCallFinished {
                id: "edit-loc-1".to_string(),
                output: "ok".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-edit-loc", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let tool_call = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::ToolCall(tc) = u {
                Some(tc)
            } else {
                None
            }
        })
        .expect("expected ToolCall notification");

    assert!(
        !tool_call.locations.is_empty(),
        "Edit tool must include at least one location"
    );
    assert_eq!(
        tool_call.locations[0].path.to_str(),
        Some("/src/main.rs"),
        "location path must match file_path input"
    );
    assert!(
        matches!(tool_call.kind, ToolKind::Edit),
        "Edit tool kind must be Edit, got: {:?}",
        tool_call.kind
    );
}

/// Read tool started notification must include the file path in locations and kind=Read.
#[tokio::test]
async fn read_tool_started_includes_file_location_and_kind() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-read-loc",
        vec![
            PromptEvent::ToolCallStarted {
                id: "read-loc-1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/src/lib.rs"}),
            },
            PromptEvent::ToolCallFinished {
                id: "read-loc-1".to_string(),
                output: "pub fn foo() {}".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-read-loc", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let tool_call = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::ToolCall(tc) = u {
                Some(tc)
            } else {
                None
            }
        })
        .expect("expected ToolCall notification");

    assert!(
        !tool_call.locations.is_empty(),
        "Read tool must include at least one location"
    );
    assert_eq!(
        tool_call.locations[0].path.to_str(),
        Some("/src/lib.rs"),
        "location path must match file_path input"
    );
    assert!(
        matches!(tool_call.kind, ToolKind::Read),
        "Read tool kind must be Read, got: {:?}",
        tool_call.kind
    );
}

// ── edit diffs ─────────────────────────────────────────────────────────────────

/// When Edit tool finishes successfully, the ToolCallUpdate must carry a Diff
/// content block with the old and new text extracted from the tool input.
#[tokio::test]
async fn edit_tool_finished_emits_diff_content_with_old_and_new_text() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-edit-diff",
        vec![
            PromptEvent::ToolCallStarted {
                id: "edit-diff-1".to_string(),
                name: "Edit".to_string(),
                input: serde_json::json!({
                    "file_path": "/src/lib.rs",
                    "old_string": "old code",
                    "new_string": "new code"
                }),
            },
            PromptEvent::ToolCallFinished {
                id: "edit-diff-1".to_string(),
                output: "success".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-edit-diff", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let update = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::ToolCallUpdate(u) = u {
                Some(u)
            } else {
                None
            }
        })
        .expect("expected ToolCallUpdate");

    let content = update
        .fields
        .content
        .as_ref()
        .expect("Edit tool update must carry content");
    assert!(
        !content.is_empty(),
        "Edit tool must produce at least one content block"
    );

    let diff = content
        .iter()
        .find_map(|c| {
            if let ToolCallContent::Diff(d) = c {
                Some(d)
            } else {
                None
            }
        })
        .expect("Edit tool content must contain a Diff block");

    assert_eq!(
        diff.path.to_str(),
        Some("/src/lib.rs"),
        "diff path must match file_path"
    );
    assert_eq!(
        diff.new_text, "new code",
        "diff new_text must match new_string input"
    );
    assert_eq!(
        diff.old_text.as_deref(),
        Some("old code"),
        "diff old_text must match old_string input"
    );

    // Also verify locations are set on the ToolCallUpdate
    let locations = update
        .fields
        .locations
        .as_ref()
        .expect("Edit update must have locations");
    assert_eq!(
        locations[0].path.to_str(),
        Some("/src/lib.rs"),
        "update location must match file_path"
    );
}

/// Read tool finished must wrap the output in a fenced code block.
#[tokio::test]
async fn read_tool_finished_wraps_output_in_fenced_code_block() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp");

    mock_runner(
        nats,
        "acp",
        "sess-read-fence",
        vec![
            PromptEvent::ToolCallStarted {
                id: "read-fence-1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/src/main.rs"}),
            },
            PromptEvent::ToolCallFinished {
                id: "read-fence-1".to_string(),
                output: "fn main() {}\n".to_string(),
                exit_code: Some(0),
                signal: None,
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-read-fence", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let update = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::ToolCallUpdate(u) = u {
                Some(u)
            } else {
                None
            }
        })
        .expect("expected ToolCallUpdate for Read");

    let content = update
        .fields
        .content
        .as_ref()
        .expect("Read update must carry content");
    assert!(
        !content.is_empty(),
        "Read must produce at least one content block"
    );

    // The content block must be a text Content (not a Diff)
    let text_content = content
        .iter()
        .find_map(|c| {
            if let ToolCallContent::Content(ct) = c {
                if let ContentBlock::Text(t) = &ct.content {
                    Some(t.text.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("Read content must be a text Content block");

    assert!(
        text_content.starts_with("```"),
        "Read output must be wrapped in a fenced code block, got: {text_content:?}"
    );
    assert!(
        text_content.contains("fn main()"),
        "fenced block must contain the file contents, got: {text_content:?}"
    );
}

// ── initialize / terminal_output_cap ──────────────────────────────────────────

/// `initialize` with `_meta.terminal_output: true` in client_capabilities must
/// flip the bridge's `terminal_output_cap` flag to `true`.
#[tokio::test]
async fn initialize_with_terminal_output_meta_sets_bridge_cap() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let mut meta = serde_json::Map::new();
    meta.insert("terminal_output".to_string(), serde_json::Value::Bool(true));
    let caps = ClientCapabilities::new().meta(meta);

    bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST).client_capabilities(caps))
        .await
        .unwrap();

    assert!(
        bridge.supports_terminal_output(),
        "bridge must have terminal_output_cap=true after initialize with terminal_output:true in _meta"
    );
}

/// `initialize` without `terminal_output` in client_capabilities must leave
/// the bridge's `terminal_output_cap` flag as `false`.
#[tokio::test]
async fn initialize_without_terminal_output_meta_leaves_cap_false() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
        .await
        .unwrap();

    assert!(
        !bridge.supports_terminal_output(),
        "bridge must have terminal_output_cap=false when terminal_output is not in _meta"
    );
}

// ── fork_session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn fork_session_returns_new_session_id() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let forked_id = SessionId::from("forked-sess-1");
    let mut agent_sub = nats.subscribe("acp.s1.agent.session.fork").await.unwrap();
    let nats2 = nats.clone();
    let resp_id = forked_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&ForkSessionResponse::new(resp_id)).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge
        .fork_session(ForkSessionRequest::new("s1", "."))
        .await;
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
    assert_eq!(result.unwrap().session_id, forked_id);
}

#[tokio::test]
async fn fork_session_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.orig-sess.agent.session.fork")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp =
                serde_json::to_vec(&ForkSessionResponse::new(SessionId::from("f1"))).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
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
    assert_eq!(subject, "acp.orig-sess.agent.session.fork");
}

#[tokio::test]
async fn fork_session_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp");

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
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats.subscribe("acp.s2.agent.session.resume").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&ResumeSessionResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge
        .resume_session(ResumeSessionRequest::new("s2", "."))
        .await;
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
}

#[tokio::test]
async fn resume_session_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.my-sess.agent.session.resume")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&ResumeSessionResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
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
    assert_eq!(subject, "acp.my-sess.agent.session.resume");
}

#[tokio::test]
async fn resume_session_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp");

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
    let bridge = make_bridge(nats.clone(), "acp");

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
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
    assert!(result.unwrap().sessions.is_empty());
}

#[tokio::test]
async fn list_sessions_uses_global_subject_not_session_scoped() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

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

    bridge
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap();

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
    let bridge = make_bridge(nats, "acp");

    let err = bridge
        .list_sessions(ListSessionsRequest::new())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

// ── set_session_model ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats
        .subscribe("acp.s3.agent.session.set_model")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&SetSessionModelResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge
        .set_session_model(SetSessionModelRequest::new("s3", "claude-sonnet-4-6"))
        .await;
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
}

#[tokio::test]
async fn set_session_model_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.sess-m.agent.session.set_model")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&SetSessionModelResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .set_session_model(SetSessionModelRequest::new("sess-m", "claude-opus-4-6"))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    assert_eq!(subject, "acp.sess-m.agent.session.set_model");
}

#[tokio::test]
async fn set_session_model_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp");

    let err = bridge
        .set_session_model(SetSessionModelRequest::new("s3", "claude-sonnet-4-6"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

// ── set_session_config_option ──────────────────────────────────────────────────

#[tokio::test]
async fn set_session_config_option_succeeds() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats
        .subscribe("acp.s4.agent.session.set_config_option")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&SetSessionConfigOptionResponse::new(vec![])).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new("s4", "mode", "plan"))
        .await;
    assert!(
        result.is_ok(),
        "expected Ok, got: {:?}",
        result.unwrap_err()
    );
}

#[tokio::test]
async fn set_session_config_option_uses_session_scoped_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.sess-cfg.agent.session.set_config_option")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&SetSessionConfigOptionResponse::new(vec![])).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new(
            "sess-cfg", "mode", "plan",
        ))
        .await
        .unwrap();

    let subject = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("timed out waiting for subject")
        .unwrap();
    assert_eq!(subject, "acp.sess-cfg.agent.session.set_config_option");
}

#[tokio::test]
async fn set_session_config_option_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp");

    let err = bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new("s4", "mode", "plan"))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}
