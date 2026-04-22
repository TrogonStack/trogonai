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

use acp_nats::prompt_event::PromptEvent;
use acp_nats::{AGENT_UNAVAILABLE, AcpPrefix, Bridge, Config, NatsAuth, NatsConfig};
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ErrorCode, ExtNotification, ExtRequest, ExtResponse,
    ForkSessionRequest, ForkSessionResponse, ImageContent, Implementation, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, ProtocolVersion,
    ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason,
    ToolCallContent, ToolCallStatus, ToolKind,
};
use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt as _, runners::AsyncRunner};
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

/// Provision only the session-scoped JetStream streams (COMMANDS, RESPONSES,
/// NOTIFICATIONS, CLIENT_OPS). Skips GLOBAL and GLOBAL_EXT intentionally:
/// global ops (initialize, authenticate, etc.) use core NATS request/reply,
/// and provisioning those streams would cause JetStream to ack the requests
/// before the mock agent responds.
async fn provision_session_streams(
    js: &trogon_nats::jetstream::NatsJetStreamClient,
    prefix: &AcpPrefix,
) {
    use acp_nats::nats::AcpStream;
    use trogon_nats::jetstream::JetStreamContext as _;
    for stream in [
        AcpStream::Commands,
        AcpStream::Responses,
        AcpStream::Notifications,
        AcpStream::ClientOps,
    ] {
        js.get_or_create_stream(stream.config(prefix))
            .await
            .unwrap_or_else(|e| panic!("failed to create {stream} stream: {e}"));
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
    .with_operation_timeout(Duration::from_secs(2))
    .with_prompt_timeout(Duration::from_secs(5));
    let js = make_js(&nats);
    provision_session_streams(&js, &acp_prefix).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Bridge::new(
        nats,
        js,
        SystemClock,
        &opentelemetry::global::meter("acp-nats-integration-test"),
        config,
        tx,
    )
}

/// Like `make_bridge` but keeps the notification receiver alive so tests can
/// assert on the `SessionNotification`s produced during a prompt.
async fn make_bridge_with_rx(
    nats: async_nats::Client,
    prefix: &str,
) -> (
    NatsBridge,
    tokio::sync::mpsc::Receiver<agent_client_protocol::SessionNotification>,
) {
    let acp_prefix = AcpPrefix::new(prefix).unwrap();
    let config = Config::new(
        acp_prefix.clone(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_secs(2))
    .with_prompt_timeout(Duration::from_secs(5));
    let js = make_js(&nats);
    provision_session_streams(&js, &acp_prefix).await;
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let bridge = Bridge::new(
        nats,
        js,
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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.agent.initialize").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats.subscribe("acp.agent.authenticate").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&AuthenticateResponse::default()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            // Respond so bridge doesn't time out.
            let resp = serde_json::to_vec(&LoadSessionResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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

    let mut agent_sub = nats
        .subscribe("acp.session.s1.agent.set_mode")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&SetSessionModeResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            let resp =
                serde_json::to_vec(&InitializeResponse::new(ProtocolVersion::LATEST)).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
/// 1. Subscribes to `{prefix}.{session_id}.agent.session.prompt`
/// 2. Reads `req_id` from the `X-Req-Id` header
/// 3. Converts each `PromptEvent` via `PromptEventConverter` into `SessionNotification`s
/// 4. Publishes notifications to `agent.session.update.{req_id}`
/// 5. On terminal outcome (Done/Error) publishes to `agent.ext.session.prompt.response.{req_id}`
///
/// Returns only after the NATS subscription is confirmed, eliminating the race
/// where the bridge publishes the prompt before the mock has subscribed.
async fn mock_runner(
    nats: async_nats::Client,
    prefix: &str,
    session_id: &str,
    events: Vec<PromptEvent>,
) {
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
                .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                .map(|v| v.as_str().to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            let update_subject =
                format!("{}.session.{}.agent.update.{}", prefix, session_id, req_id);
            let response_subject = format!(
                "{}.session.{}.agent.prompt.response.{}",
                prefix, session_id, req_id
            );

            let mut converter = PromptEventConverter::new(session_id.clone());
            for event in events {
                let (notifications, outcome) = converter.convert(event);
                for notif in &notifications {
                    nats.publish(
                        update_subject.clone(),
                        serde_json::to_vec(notif).unwrap().into(),
                    )
                    .await
                    .unwrap();
                }
                if !notifications.is_empty() {
                    nats.flush().await.unwrap();
                }
                if let Some(outcome) = outcome {
                    match outcome {
                        PromptOutcome::Done { stop_reason } => {
                            let resp = agent_client_protocol::PromptResponse::new(
                                parse_stop_reason(&stop_reason),
                            );
                            nats.publish(
                                response_subject,
                                serde_json::to_vec(&resp).unwrap().into(),
                            )
                            .await
                            .unwrap();
                        }
                        PromptOutcome::Error { message } => {
                            let env = serde_json::json!({"error": message});
                            nats.publish(
                                response_subject,
                                serde_json::to_vec(&env).unwrap().into(),
                            )
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
                .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
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
    assert!(result.is_err(), "expected Err from malformed event JSON");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("timed out"),
        "expected prompt to time out after bad notification payload",
    );
}

// ── prompt / tool calls ───────────────────────────────────────────────────────

#[tokio::test]
async fn tool_call_started_produces_tool_call_in_progress_notification() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-tc-start",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-1".to_string(),
                name: "get_pr_diff".to_string(),
                input: serde_json::json!({"owner": "acme", "repo": "api", "pr_number": 42}),
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-tc-ok",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-ok".to_string(),
                name: "list_pr_files".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-tc-fail",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-fail".to_string(),
                name: "update_file".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-tc-sig",
        vec![
            PromptEvent::ToolCallStarted {
                id: "call-sig".to_string(),
                name: "post_pr_comment".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
                parent_tool_use_id: None,
            },
            PromptEvent::ToolCallStarted {
                id: "dup-id".to_string(),
                name: "get_pr_diff".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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

/// Without terminal_output_cap, a Bash tool produces the normal 1 ToolCall + 1 ToolCallUpdate.
#[tokio::test]
async fn bash_without_terminal_output_cap_emits_standard_two_notifications() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;
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
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-read-loc",
        vec![
            PromptEvent::ToolCallStarted {
                id: "read-loc-1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/src/lib.rs"}),
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

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
                parent_tool_use_id: None,
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
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-read-fence",
        vec![
            PromptEvent::ToolCallStarted {
                id: "read-fence-1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/src/main.rs"}),
                parent_tool_use_id: None,
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
            let resp = serde_json::to_vec(&ForkSessionResponse::new(resp_id)).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.session.orig-sess.agent.fork")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp =
                serde_json::to_vec(&ForkSessionResponse::new(SessionId::from("f1"))).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            let resp = serde_json::to_vec(&ResumeSessionResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.session.my-sess.agent.resume")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&ResumeSessionResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats.subscribe("acp.agent.session.list").await.unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&ListSessionsResponse::new(vec![])).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
    let bridge = make_bridge(nats, "acp").await;

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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats
        .subscribe("acp.session.s3.agent.set_model")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&SetSessionModelResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let mut agent_sub = nats
        .subscribe("acp.session.sess-m.agent.set_model")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&SetSessionModelResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
    assert_eq!(subject, "acp.session.sess-m.agent.set_model");
}

#[tokio::test]
async fn set_session_model_timeout_returns_agent_unavailable() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let bridge = make_bridge(nats, "acp").await;

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
    let bridge = make_bridge(nats.clone(), "acp").await;

    let mut agent_sub = nats
        .subscribe("acp.session.s4.agent.set_config_option")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&SetSessionConfigOptionResponse::new(vec![])).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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
            let resp = serde_json::to_vec(&SetSessionConfigOptionResponse::new(vec![])).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
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

/// A `SystemStatus` message containing "compact" (but not "complet") is mapped
/// to an `AgentMessageChunk` with text `"Compacting..."`.
#[tokio::test]
async fn system_status_compact_emits_agent_message_chunk() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-sys-compact",
        vec![
            PromptEvent::ToolCallStarted {
                id: "tc-sys-1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: None,
            },
            PromptEvent::SystemStatus {
                message: "compacting memory...".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-sys-compact", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let has_compacting_chunk = updates.iter().any(|u| {
        if let SessionUpdate::AgentMessageChunk(chunk) = u
            && let ContentBlock::Text(t) = &chunk.content
        {
            return t.text.contains("Compacting...");
        }
        false
    });
    assert!(
        has_compacting_chunk,
        "expected AgentMessageChunk with 'Compacting...' text; got: {updates:?}"
    );
}

/// A `SystemStatus` message containing "compact complete" is mapped to an
/// `AgentMessageChunk` with text `"\n\nCompacting completed."`.
#[tokio::test]
async fn system_status_compacting_complete_emits_completed_message() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-sys-complete",
        vec![
            PromptEvent::SystemStatus {
                message: "compact complete".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-sys-complete", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    let has_completed_chunk = updates.iter().any(|u| {
        if let SessionUpdate::AgentMessageChunk(chunk) = u
            && let ContentBlock::Text(t) = &chunk.content
        {
            return t.text.contains("Compacting completed.");
        }
        false
    });
    assert!(
        has_completed_chunk,
        "expected AgentMessageChunk with 'Compacting completed.' text; got: {updates:?}"
    );
}

/// A `SystemStatus` message that does not contain "compact" must NOT produce
/// any `AgentMessageChunk` notification.
#[tokio::test]
async fn system_status_non_compact_does_not_emit_agent_message() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-sys-ratelimit",
        vec![
            PromptEvent::SystemStatus {
                message: "rate_limit_warning".to_string(),
            },
            PromptEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    )
    .await;

    bridge
        .prompt(PromptRequest::new("sess-sys-ratelimit", vec![]))
        .await
        .unwrap();

    let updates = drain_updates(&mut rx);
    // Filter for any AgentMessageChunk that contains "Compacting"
    let compacting_chunks: Vec<_> = updates
        .iter()
        .filter(|u| {
            if let SessionUpdate::AgentMessageChunk(chunk) = u
                && let ContentBlock::Text(t) = &chunk.content
            {
                return t.text.contains("Compacting");
            }
            false
        })
        .collect();
    assert!(
        compacting_chunks.is_empty(),
        "non-compact SystemStatus must not produce an AgentMessageChunk with 'Compacting'; got: {compacting_chunks:?}"
    );
}

/// A `ToolCallStarted` with a `parent_tool_use_id` must inject
/// `meta.claudeCode.parentToolUseId` into the `ToolCall` notification.
#[tokio::test]
async fn tool_call_started_with_parent_id_injects_meta() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-parent-meta",
        vec![
            PromptEvent::ToolCallStarted {
                id: "tc-1".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: Some("parent-tu-123".to_string()),
            },
            PromptEvent::ToolCallFinished {
                id: "tc-1".to_string(),
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
        .prompt(PromptRequest::new("sess-parent-meta", vec![]))
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
    let tool_call = tool_call.expect("expected at least one ToolCall notification");

    let meta = tool_call
        .meta
        .as_ref()
        .expect("tool_call.meta must be Some");
    let parent_id = meta
        .get("claudeCode")
        .and_then(|cc| cc.get("parentToolUseId"))
        .and_then(|v| v.as_str());

    assert_eq!(
        parent_id,
        Some("parent-tu-123"),
        "meta.claudeCode.parentToolUseId must equal 'parent-tu-123'; meta: {meta:?}"
    );
}

/// A `ToolCallStarted` without a `parent_tool_use_id` must NOT include
/// `parentToolUseId` inside `meta.claudeCode`.
#[tokio::test]
async fn tool_call_started_without_parent_id_no_meta_field() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, mut rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    mock_runner(
        nats,
        "acp",
        "sess-no-parent-meta",
        vec![
            PromptEvent::ToolCallStarted {
                id: "tc-2".to_string(),
                name: "Bash".to_string(),
                input: serde_json::json!({}),
                parent_tool_use_id: None,
            },
            PromptEvent::ToolCallFinished {
                id: "tc-2".to_string(),
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
        .prompt(PromptRequest::new("sess-no-parent-meta", vec![]))
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
    let tool_call = tool_call.expect("expected at least one ToolCall notification");

    // If claudeCode exists in meta, it must not contain parentToolUseId
    if let Some(meta) = &tool_call.meta
        && let Some(cc) = meta.get("claudeCode")
    {
        assert!(
            cc.get("parentToolUseId").is_none(),
            "parentToolUseId must not appear when parent_tool_use_id is None; meta: {meta:?}"
        );
    }
}

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
    let mut agent_sub = nats
        .subscribe("acp.session.src-sess-1.agent.fork")
        .await
        .unwrap();
    let nats2 = nats.clone();
    let resp_id = forked_id.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&ForkSessionResponse::new(resp_id)).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
            }
        }
    });

    let result = bridge
        .fork_session(ForkSessionRequest::new("src-sess-1", "."))
        .await;
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
    let mut agent_sub = nats
        .subscribe("acp.session.fork-src-2.agent.fork")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&ForkSessionResponse::new(SessionId::from("fork-dst-2")))
                .unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
            }
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

    let mut agent_sub = nats
        .subscribe("acp.session.resume-sess-1.agent.resume")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let resp = serde_json::to_vec(&ResumeSessionResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
            }
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
    let mut agent_sub = nats
        .subscribe("acp.session.resume-sess-2.agent.resume")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = agent_sub.next().await {
            let _ = tx.send(msg.subject.to_string());
            let resp = serde_json::to_vec(&ResumeSessionResponse::new()).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp.into()).await.unwrap();
                }
            }
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
        agent_client_protocol::ImageContent::new("", "image/jpeg")
            .uri("https://example.com/photo.jpg".to_string()),
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
            let raw =
                serde_json::value::RawValue::from_string(r#"{"status":"closed"}"#.to_string())
                    .unwrap();
            let resp = ExtResponse::new(raw.into());
            let resp_bytes = serde_json::to_vec(&resp).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp_bytes.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp_bytes.into()).await.unwrap();
                }
            }
        }
    });

    let params =
        serde_json::value::RawValue::from_string(r#"{"sessionId":"sess-ext-1"}"#.to_string())
            .unwrap();
    let result = bridge
        .ext_method(ExtRequest::new("session_close", params.into()))
        .await;

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

    let params =
        serde_json::value::RawValue::from_string(r#"{"event":"ping"}"#.to_string()).unwrap();
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
    assert!(
        result.is_ok(),
        "fire-and-forget: must be Ok even with no subscriber"
    );
}

// ── close_session integration ─────────────────────────────────────────────────

/// close_session through the bridge routes to the correct per-session NATS subject.
#[tokio::test]
async fn close_session_integration_forwards_request_and_returns_response() {
    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let (bridge, _rx) = make_bridge_with_rx(nats.clone(), "acp").await;

    let mut sub = nats
        .subscribe("acp.session.sess-close-1.agent.close")
        .await
        .unwrap();
    let nats2 = nats.clone();
    tokio::spawn(async move {
        if let Some(msg) = sub.next().await {
            let resp = CloseSessionResponse::new();
            let resp_bytes = serde_json::to_vec(&resp).unwrap();
            {
                let req_id = msg
                    .headers
                    .as_ref()
                    .and_then(|h| h.get(acp_nats::REQ_ID_HEADER))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default();
                let is_session_op = {
                    let parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    parts.get(1) == Some(&"session")
                };
                if is_session_op && !req_id.is_empty() {
                    let subj_parts: Vec<&str> = msg.subject.as_str().split('.').collect();
                    let sess_id = subj_parts.get(2).copied().unwrap_or("unknown");
                    let response_subject =
                        format!("acp.session.{}.agent.response.{}", sess_id, req_id);
                    nats2.publish(response_subject, resp_bytes.into()).await.unwrap();
                } else if let Some(reply) = msg.reply {
                    nats2.publish(reply, resp_bytes.into()).await.unwrap();
                }
            }
        }
    });

    let result = bridge
        .close_session(CloseSessionRequest::new("sess-close-1"))
        .await;
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
