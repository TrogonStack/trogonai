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

use acp_nats::{AGENT_UNAVAILABLE, AcpPrefix, Bridge, Config, NatsAuth, NatsConfig};
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    CloseSessionRequest, CloseSessionResponse, ErrorCode, ExtRequest, ExtResponse,
    ForkSessionRequest, ForkSessionResponse, Implementation, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    LogoutRequest, LogoutResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, ProtocolVersion, ResumeSessionRequest, ResumeSessionResponse, SessionId,
    SetSessionModeRequest, SetSessionModeResponse, StopReason,
};
use async_nats::jetstream;
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::time::SystemClock;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Provision all ACP JetStream streams on a real NATS server.
async fn setup_streams(js: &jetstream::Context) {
    let prefix = AcpPrefix::new("acp").unwrap();
    for config in acp_nats::jetstream::streams::all_configs(&prefix) {
        js.get_or_create_stream(config).await.unwrap();
    }
}

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["-js"])
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

fn make_bridge(
    nats: async_nats::Client,
    prefix: &str,
) -> Bridge<async_nats::Client, SystemClock, NatsJetStreamClient> {
    let js = NatsJetStreamClient::new(jetstream::new(nats.clone()));
    let config = Config::new(
        AcpPrefix::new(prefix).unwrap(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500));
    Bridge::new(
        nats,
        js,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-integration-test"),
        config,
        tokio::sync::mpsc::channel(1).0,
    )
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
    let ready_subject = format!("acp.session.{}.agent.ext.ready", session_id);
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
    let js = jetstream::new(nats.clone());
    setup_streams(&js).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    // JetStream-published messages are also delivered to core NATS subscribers.
    let mut agent_sub = nats.subscribe("acp.session.s1.agent.load").await.unwrap();
    let js2 = js.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let subject = msg.subject.to_string();
            let _ = tx.send(subject);
            // Respond via JetStream so the bridge's RESPONSES consumer picks it up.
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let resp_subject = format!("acp.session.s1.agent.response.{req_id}");
            let resp = serde_json::to_vec(&LoadSessionResponse::new()).unwrap();
            let _ = js2.publish(resp_subject, resp.into()).await;
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

    assert_eq!(subject, "acp.session.s1.agent.load");
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
    let js = jetstream::new(nats.clone());
    setup_streams(&js).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let session_id = "ls-ready-test";
    let ready_subject = format!("acp.session.{}.agent.ext.ready", session_id);
    let mut ready_sub = nats.subscribe(ready_subject.clone()).await.unwrap();

    let mut agent_sub = nats
        .subscribe(format!("acp.session.{}.agent.load", session_id))
        .await
        .unwrap();
    let js2 = js.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let resp_subject = format!("acp.session.{session_id}.agent.response.{req_id}");
            let resp = serde_json::to_vec(&LoadSessionResponse::new()).unwrap();
            let _ = js2.publish(resp_subject, resp.into()).await;
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
    let js = jetstream::new(nats.clone());
    setup_streams(&js).await;
    let bridge = make_bridge(nats.clone(), "acp");

    // JetStream-published messages are also delivered to core NATS subscribers.
    let mut agent_sub = nats
        .subscribe("acp.session.s1.agent.set_mode")
        .await
        .unwrap();
    let js2 = js.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let resp_subject = format!("acp.session.s1.agent.response.{req_id}");
            let resp = serde_json::to_vec(&SetSessionModeResponse::new()).unwrap();
            let _ = js2.publish(resp_subject, resp.into()).await;
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

// ── prompt ────────────────────────────────────────────────────────────────────

/// `Bridge::prompt` publishes to the JetStream COMMANDS stream, waits on
/// consumers for the NOTIFICATIONS and RESPONSES streams, and returns a
/// `PromptResponse` once a runner publishes the response.
///
/// The fake runner subscribes to core NATS (JetStream delivers to core NATS
/// subscribers too), reads the `X-Req-Id` header, and JetStream-publishes the
/// response to `{prefix}.session.{id}.agent.prompt.response.{req_id}`.
#[tokio::test]
async fn prompt_succeeds_with_real_jetstream() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let js = jetstream::new(nats.clone());
    setup_streams(&js).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let session_id = "prompt-integ-sess";

    // Subscribe BEFORE calling bridge.prompt() so the JetStream publish is received.
    let mut agent_sub = nats
        .subscribe(format!("acp.session.{}.agent.prompt", session_id))
        .await
        .unwrap();
    let js2 = js.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get("X-Req-Id"))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let resp_subject = format!(
                "acp.session.{}.agent.prompt.response.{}",
                session_id, req_id
            );
            let resp = serde_json::to_vec(&PromptResponse::new(StopReason::EndTurn)).unwrap();
            // JetStream-publish so the bridge's RESPONSES consumer picks it up.
            let _ = js2.publish(resp_subject, resp.into()).await;
        }
    });

    let result = bridge
        .prompt(PromptRequest::new(session_id, vec![]))
        .await;

    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
    assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
}

// ── list_sessions ─────────────────────────────────────────────────────────────

/// `Bridge::list_sessions` sends a core-NATS request to `{prefix}.agent.session.list`
/// and returns the `ListSessionsResponse` published by the agent.
#[tokio::test]
async fn list_sessions_routes_to_agent_and_returns_response() {
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
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

// ── logout ────────────────────────────────────────────────────────────────────

/// `Bridge::logout` sends a core-NATS request to `{prefix}.agent.logout` and
/// returns the `LogoutResponse` published by the agent.
#[tokio::test]
async fn logout_routes_to_agent_and_returns_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats.subscribe("acp.agent.logout").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&LogoutResponse::new()).unwrap();
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp.into()).await.unwrap();
            }
        }
    });

    let result = bridge.logout(LogoutRequest::new()).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

// ── fork_session ──────────────────────────────────────────────────────────────

/// `Bridge::fork_session` JetStream-publishes to the COMMANDS stream, waits on
/// a RESPONSES consumer, and returns `ForkSessionResponse` once the runner
/// JetStream-publishes the response.
#[tokio::test]
async fn fork_session_succeeds_with_real_jetstream() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let js = jetstream::new(nats.clone());
    setup_streams(&js).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let session_id = "fork-integ-sess";
    let new_session_id = SessionId::from("forked-session-1");

    let mut agent_sub = nats
        .subscribe(format!("acp.session.{}.agent.fork", session_id))
        .await
        .unwrap();
    let js2 = js.clone();
    let new_id = new_session_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let resp_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);
            let resp = serde_json::to_vec(&ForkSessionResponse::new(new_id)).unwrap();
            let _ = js2.publish(resp_subject, resp.into()).await;
        }
    });

    let result = bridge
        .fork_session(ForkSessionRequest::new(session_id, "."))
        .await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
    assert_eq!(result.unwrap().session_id, new_session_id);
}

// ── close_session ─────────────────────────────────────────────────────────────

/// `Bridge::close_session` JetStream-publishes to the COMMANDS stream and returns
/// `CloseSessionResponse` once the runner JetStream-publishes the response.
#[tokio::test]
async fn close_session_succeeds_with_real_jetstream() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let js = jetstream::new(nats.clone());
    setup_streams(&js).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let session_id = "close-integ-sess";

    let mut agent_sub = nats
        .subscribe(format!("acp.session.{}.agent.close", session_id))
        .await
        .unwrap();
    let js2 = js.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let resp_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);
            let resp = serde_json::to_vec(&CloseSessionResponse::new()).unwrap();
            let _ = js2.publish(resp_subject, resp.into()).await;
        }
    });

    let result = bridge
        .close_session(CloseSessionRequest::new(session_id))
        .await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

// ── resume_session ────────────────────────────────────────────────────────────

/// `Bridge::resume_session` JetStream-publishes to the COMMANDS stream and returns
/// `ResumeSessionResponse` once the runner JetStream-publishes the response.
#[tokio::test]
async fn resume_session_succeeds_with_real_jetstream() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let js = jetstream::new(nats.clone());
    setup_streams(&js).await;
    let bridge = make_bridge(nats.clone(), "acp");

    let session_id = "resume-integ-sess";

    let mut agent_sub = nats
        .subscribe(format!("acp.session.{}.agent.resume", session_id))
        .await
        .unwrap();
    let js2 = js.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let req_id = msg
                .headers
                .as_ref()
                .and_then(|h| h.get(trogon_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_default();
            let resp_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);
            let resp = serde_json::to_vec(&ResumeSessionResponse::new()).unwrap();
            let _ = js2.publish(resp_subject, resp.into()).await;
        }
    });

    let result = bridge
        .resume_session(ResumeSessionRequest::new(session_id, "."))
        .await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}

// ── ext_method ────────────────────────────────────────────────────────────────

/// `Bridge::ext_method` sends a core-NATS request to `{prefix}.agent.ext.{method}`
/// and returns the `ExtResponse` published by the agent.  This exercises the full
/// client→NATS→agent round-trip for extension methods.
#[tokio::test]
async fn ext_method_routes_to_agent_and_returns_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    // No JetStream stream setup required — ext_method uses core NATS request-reply.
    let bridge = make_bridge(nats.clone(), "acp");

    let mut agent_sub = nats.subscribe("acp.agent.ext.ping").await.unwrap();
    let nats2 = nats.clone();
    let resp_payload =
        serde_json::to_vec(&ExtResponse::new(
            serde_json::value::RawValue::from_string(r#"{"pong":true}"#.to_string())
                .unwrap()
                .into(),
        ))
        .unwrap();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            if let Some(reply) = msg.reply {
                nats2.publish(reply, resp_payload.into()).await.unwrap();
            }
        }
    });

    let params =
        serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    let result = bridge
        .ext_method(ExtRequest::new("ping", params.into()))
        .await;

    assert!(result.is_ok(), "expected Ok, got: {:?}", result.unwrap_err());
}
