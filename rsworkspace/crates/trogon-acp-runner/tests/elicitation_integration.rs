//! Integration tests for `handle_elicitation_request_nats` — requires Docker.
//!
//! These tests exercise the full NATS request-reply round-trip with a real NATS
//! server, unlike the unit tests in `elicitation.rs` which use a mock client.
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test elicitation_integration

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    ElicitationAcceptAction, ElicitationAction, ElicitationContentValue, ElicitationFormMode,
    ElicitationMode, ElicitationRequest, ElicitationResponse, ElicitationSchema,
};
use bytes::Bytes;
use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};
use tokio::sync::oneshot;
use trogon_acp_runner::{elicitation::handle_elicitation_request_nats, ElicitationReq};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (c, nats)
}

const PREFIX: &str = "acp";
const SESSION: &str = "sess-elic-1";

fn subject() -> String {
    format!("{PREFIX}.session.{SESSION}.client.session.elicitation")
}

fn make_req() -> (ElicitationReq, oneshot::Receiver<agent_client_protocol::Result<ElicitationResponse>>) {
    let (tx, rx) = oneshot::channel();
    let request = ElicitationRequest::new(
        SESSION.to_string(),
        ElicitationMode::Form(ElicitationFormMode::new(ElicitationSchema::new().string("answer", true))),
        "What is your preferred language?",
    );
    let req = ElicitationReq { request, response_tx: tx };
    (req, rx)
}

fn accept_response(answer: &str) -> Bytes {
    let mut content = std::collections::BTreeMap::new();
    content.insert(
        "answer".to_string(),
        ElicitationContentValue::String(answer.to_string()),
    );
    let resp = ElicitationResponse::new(ElicitationAction::Accept(
        ElicitationAcceptAction::new().content(content),
    ));
    serde_json::to_vec(&resp).unwrap().into()
}

fn cancel_response() -> Bytes {
    serde_json::to_vec(&ElicitationResponse::new(ElicitationAction::Cancel))
        .unwrap()
        .into()
}

/// Subscribe to the elicitation subject and reply with `reply_bytes`.
fn spawn_responder(nats: async_nats::Client, reply_bytes: Bytes) {
    tokio::spawn(async move {
        let mut sub = nats.subscribe(subject()).await.unwrap();
        if let Some(msg) = sub.next().await {
            if let Some(reply) = msg.reply {
                nats.publish(reply, reply_bytes).await.unwrap();
            }
        }
    });
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `Accept` response over real NATS → Ok(response) forwarded to caller.
#[tokio::test]
async fn accept_response_forwarded_over_real_nats() {
    let (_c, nats) = start_nats().await;

    spawn_responder(nats.clone(), accept_response("Rust"));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (req, rx) = make_req();
    handle_elicitation_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap()).await;

    let result = rx.await.unwrap();
    assert!(result.is_ok(), "must be Ok; got: {result:?}");
    let resp = result.unwrap();
    assert!(
        matches!(resp.action, ElicitationAction::Accept(_)),
        "action must be Accept"
    );
}

/// `Cancel` response over real NATS → Ok(Cancel) forwarded to caller.
#[tokio::test]
async fn cancel_response_forwarded_over_real_nats() {
    let (_c, nats) = start_nats().await;

    spawn_responder(nats.clone(), cancel_response());
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (req, rx) = make_req();
    handle_elicitation_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap()).await;

    let result = rx.await.unwrap();
    assert!(result.is_ok(), "must be Ok; got: {result:?}");
    assert!(
        matches!(result.unwrap().action, ElicitationAction::Cancel),
        "action must be Cancel"
    );
}

/// No subscriber → NATS error → Err forwarded to caller.
#[tokio::test]
async fn nats_error_forwarded_as_err_over_real_nats() {
    let (_c, nats) = start_nats().await;

    // No responder — NATS will return no-responder error immediately.
    let (req, rx) = make_req();
    handle_elicitation_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap()).await;

    let result = rx.await.unwrap();
    assert!(result.is_err(), "network error must produce Err; got: {result:?}");
}

/// Verifies the NATS request payload contains the question text.
#[tokio::test]
async fn request_payload_contains_question_text() {
    let (_c, nats) = start_nats().await;

    let nats_clone = nats.clone();
    let captured: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let mut sub = nats_clone.subscribe(subject()).await.unwrap();
        if let Some(msg) = sub.next().await {
            let body: serde_json::Value =
                serde_json::from_slice(&msg.payload).unwrap_or_default();
            *captured_clone.lock().unwrap() = Some(body);
            if let Some(reply) = msg.reply {
                nats_clone.publish(reply, cancel_response()).await.unwrap();
            }
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let (tx, _rx) = oneshot::channel();
    let request = ElicitationRequest::new(
        SESSION.to_string(),
        ElicitationMode::Form(ElicitationFormMode::new(ElicitationSchema::new())),
        "unique-question-marker-xyz",
    );
    let req = ElicitationReq { request, response_tx: tx };
    handle_elicitation_request_nats(req, nats, AcpPrefix::new(PREFIX).unwrap()).await;

    let payload = captured.lock().unwrap().clone().expect("payload captured");
    let payload_str = payload.to_string();
    assert!(
        payload_str.contains("unique-question-marker-xyz"),
        "payload must contain the question; got: {payload_str}"
    );
}
