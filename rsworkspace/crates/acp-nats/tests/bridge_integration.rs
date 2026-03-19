//! Integration tests for acp-nats Bridge with a real NATS server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).
//!
//! Run with:
//!   cargo test -p acp-nats --test bridge_integration

use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
use std::time::Duration;
use std::collections::HashSet;

use acp_nats::{AcpPrefix, Bridge, Config, NatsAuth, NatsConfig, AGENT_UNAVAILABLE};
use acp_nats::prompt_event::PromptEvent;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, ErrorCode,
    Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, ProtocolVersion, SessionId,
    SessionUpdate, SetSessionModeRequest, SetSessionModeResponse,
};
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};
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

    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
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
        if let Some(msg) = agent_sub.next().await {
            if let Some(reply) = msg.reply {
                // Send malformed JSON.
                nats2.publish(reply, b"{bad json}".as_ref().into()).await.unwrap();
            }
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

    let result = bridge.authenticate(AuthenticateRequest::new("password")).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
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
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
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

    bridge.new_session(NewSessionRequest::new(".")).await.unwrap();

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

    bridge.load_session(LoadSessionRequest::new("s1", ".")).await.unwrap();

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
    let result =
        tokio::time::timeout(Duration::from_millis(100), should_not_receive.next()).await;
    assert!(result.is_err(), "no NATS message should be sent for invalid session IDs");
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
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

// ── cancel ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_publishes_to_correct_subject() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut sub = nats
        .subscribe("acp.s1.agent.session.cancel")
        .await
        .unwrap();

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

    let result =
        tokio::time::timeout(Duration::from_millis(100), should_not_receive.next()).await;
    assert!(result.is_err(), "no NATS message should be published for invalid session IDs");
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
        assert!(result.is_ok(), "concurrent request failed: {:?}", result.unwrap_err());
        let id = result.unwrap().session_id.to_string();
        assert!(!id.is_empty(), "session_id must not be empty");
        session_ids.insert(id);
    }

    // All 5 should have received distinct session IDs — no reply cross-mixing.
    assert_eq!(session_ids.len(), 5, "all 5 concurrent sessions should have distinct IDs");
}

// ── prompt / ModeChanged ──────────────────────────────────────────────────────

/// A mock runner subscribes to prompt messages, extracts the `req_id` from the
/// payload, then publishes `ModeChanged` + `Done` back on the events subject.
/// The bridge must forward `CurrentModeUpdate` then `ConfigOptionUpdate`
/// to the notification channel.
#[tokio::test]
async fn mode_changed_event_produces_current_mode_and_config_option_notifications() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut notification_rx) = make_bridge_with_rx(nats.clone(), "acp");

    let session_id = "sess-mode-test";

    // Mock runner: subscribe to any prompt on this session, reply with events.
    let mut prompt_sub = nats
        .subscribe(format!("acp.{}.agent.prompt", session_id))
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = prompt_sub.next().await {
            let payload: acp_nats::prompt_event::PromptPayload =
                serde_json::from_slice(&msg.payload).expect("valid prompt payload");
            let req_id = payload.req_id;
            let events_subject =
                format!("acp.{}.agent.prompt.events.{}", session_id, req_id);

            let mode_changed = PromptEvent::ModeChanged {
                mode: "plan".to_string(),
                model: "claude-opus-4-6".to_string(),
            };
            let done = PromptEvent::Done { stop_reason: "end_turn".to_string() };
            nats2
                .publish(events_subject.clone(), serde_json::to_vec(&mode_changed).unwrap().into())
                .await
                .unwrap();
            nats2
                .publish(events_subject, serde_json::to_vec(&done).unwrap().into())
                .await
                .unwrap();
        }
    });

    // Drive the prompt to completion.
    bridge
        .prompt(PromptRequest::new(session_id, vec![]))
        .await
        .expect("prompt must succeed");

    // Collect all notifications (non-blocking drain — prompt already returned).
    let mut updates = Vec::new();
    while let Ok(n) = notification_rx.try_recv() {
        updates.push(n.update);
    }

    let has_mode = updates.iter().any(|u| matches!(u, SessionUpdate::CurrentModeUpdate(_)));
    let has_config = updates.iter().any(|u| matches!(u, SessionUpdate::ConfigOptionUpdate(_)));

    assert!(has_mode, "expected CurrentModeUpdate in notifications, got: {updates:?}");
    assert!(has_config, "expected ConfigOptionUpdate in notifications, got: {updates:?}");

    // CurrentModeUpdate must appear before ConfigOptionUpdate.
    let mode_pos = updates
        .iter()
        .position(|u| matches!(u, SessionUpdate::CurrentModeUpdate(_)))
        .unwrap();
    let config_pos = updates
        .iter()
        .position(|u| matches!(u, SessionUpdate::ConfigOptionUpdate(_)))
        .unwrap();
    assert!(mode_pos < config_pos, "CurrentModeUpdate must come before ConfigOptionUpdate");
}

/// Verify that the `CurrentModeUpdate` carries the correct mode value.
#[tokio::test]
async fn mode_changed_current_mode_update_carries_plan_mode() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut notification_rx) = make_bridge_with_rx(nats.clone(), "acp");

    let session_id = "sess-mode-value";

    let mut prompt_sub = nats
        .subscribe(format!("acp.{}.agent.prompt", session_id))
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = prompt_sub.next().await {
            let payload: acp_nats::prompt_event::PromptPayload =
                serde_json::from_slice(&msg.payload).expect("valid prompt payload");
            let events_subject =
                format!("acp.{}.agent.prompt.events.{}", session_id, payload.req_id);
            for event in [
                PromptEvent::ModeChanged { mode: "plan".to_string(), model: "claude-sonnet-4-6".to_string() },
                PromptEvent::Done { stop_reason: "end_turn".to_string() },
            ] {
                nats2
                    .publish(events_subject.clone(), serde_json::to_vec(&event).unwrap().into())
                    .await
                    .unwrap();
            }
        }
    });

    bridge.prompt(PromptRequest::new(session_id, vec![])).await.unwrap();

    let mut updates = Vec::new();
    while let Ok(n) = notification_rx.try_recv() {
        updates.push(n.update);
    }

    let mode_update = updates
        .iter()
        .find_map(|u| {
            if let SessionUpdate::CurrentModeUpdate(m) = u { Some(m) } else { None }
        })
        .expect("must have CurrentModeUpdate");

    assert_eq!(mode_update.current_mode_id.0.as_ref(), "plan");
}

/// Without a `ModeChanged` event (plain `Done` response) no mode/config
/// notifications must be sent.
#[tokio::test]
async fn no_mode_notifications_when_runner_does_not_emit_mode_changed() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut notification_rx) = make_bridge_with_rx(nats.clone(), "acp");

    let session_id = "sess-no-mode";

    let mut prompt_sub = nats
        .subscribe(format!("acp.{}.agent.prompt", session_id))
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = prompt_sub.next().await {
            let payload: acp_nats::prompt_event::PromptPayload =
                serde_json::from_slice(&msg.payload).expect("valid prompt payload");
            let events_subject =
                format!("acp.{}.agent.prompt.events.{}", session_id, payload.req_id);
            let done = PromptEvent::Done { stop_reason: "end_turn".to_string() };
            nats2
                .publish(events_subject, serde_json::to_vec(&done).unwrap().into())
                .await
                .unwrap();
        }
    });

    bridge.prompt(PromptRequest::new(session_id, vec![])).await.unwrap();

    let mut updates = Vec::new();
    while let Ok(n) = notification_rx.try_recv() {
        updates.push(n.update);
    }

    assert!(
        !updates.iter().any(|u| matches!(u, SessionUpdate::CurrentModeUpdate(_))),
        "must NOT have CurrentModeUpdate without ModeChanged event",
    );
    assert!(
        !updates.iter().any(|u| matches!(u, SessionUpdate::ConfigOptionUpdate(_))),
        "must NOT have ConfigOptionUpdate without ModeChanged event",
    );
}
