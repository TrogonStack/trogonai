//! Cross-runner integration tests: export/import between xai, openrouter, and acp runners.
//!
//! Verifies that the portable session format is compatible between all runner pairs.
//! xai↔openrouter tests use only mock HTTP clients (no network, no Docker).
//! acp-runner tests require Docker (testcontainers spins up a NATS JetStream server).

use std::sync::Arc;

use agent_client_protocol::{Agent as _, ExtRequest};
use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::RwLock;
use trogon_acp_runner::{
    GatewayConfig, NatsSessionStore, SessionState, SessionStore, TrogonAgent,
    agent_runner::mock::MockAgentRunner,
    session_notifier::mock::MockSessionNotifier,
};
use trogon_agent_core::agent_loop::{ContentBlock as AgentContentBlock, Message as AgentMessage};
use trogon_runner_tools::portable_session::PortableMessage;

use trogon_xai_runner::{
    Message as XaiMessage, MockSessionNotifier as XaiMockNotifier, MockXaiHttpClient, XaiAgent,
};

use trogon_openrouter_runner::{
    MockOpenRouterHttpClient, MockSessionNotifier as OrMockNotifier, OpenRouterAgent,
};

fn local() -> tokio::task::LocalSet {
    tokio::task::LocalSet::new()
}

// ── acp helpers ───────────────────────────────────────────────────────────────

async fn start_nats_js() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn make_js(port: u16) -> (async_nats::Client, jetstream::Context) {
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (nats, js)
}

type AcpTestAgent = TrogonAgent<NatsSessionStore, MockAgentRunner, MockSessionNotifier>;

fn make_acp_agent(store: NatsSessionStore) -> AcpTestAgent {
    TrogonAgent::new(
        MockSessionNotifier::new(),
        store,
        MockAgentRunner::new("claude-test"),
        "acp",
        "claude-test",
        None,
        None,
        Arc::new(RwLock::new(None::<GatewayConfig>)),
    )
}

#[tokio::test]
async fn cross_runner_xai_export_into_openrouter_import() {
    // ── 1. Build xai agent and seed a session with two messages ─────────────────
    let xai_http = Arc::new(MockXaiHttpClient::new());
    let xai_notifier = Arc::new(XaiMockNotifier::new());
    let xai_agent = XaiAgent::with_deps(xai_notifier, "grok-3", "test-key", xai_http);

    xai_agent
        .test_insert_session_with_history(
            "xai-s1",
            "/tmp",
            vec![
                XaiMessage::user("question"),
                XaiMessage::assistant_text("answer"),
            ],
        )
        .await;

    // ── 2. Export from xai-runner ────────────────────────────────────────────────
    let export_params = serde_json::value::RawValue::from_string(
        serde_json::json!({ "sessionId": "xai-s1" }).to_string(),
    )
    .unwrap();
    let xai_export_resp = xai_agent
        .ext_method(ExtRequest::new("session/export", Arc::from(export_params)))
        .await
        .expect("xai session/export should succeed");

    let exported_json = xai_export_resp.0.get().to_string();

    // Sanity-check the xai export
    let xai_portable: Vec<PortableMessage> =
        serde_json::from_str(&exported_json).expect("xai export should be valid JSON");
    assert_eq!(xai_portable.len(), 2);
    assert_eq!(xai_portable[0].role, "user");
    assert_eq!(xai_portable[0].text, "question");
    assert_eq!(xai_portable[1].role, "assistant");
    assert_eq!(xai_portable[1].text, "answer");

    // ── 3. Build openrouter agent (inside LocalSet because it is !Send) ──────────
    local()
        .run_until(async move {
            let or_http = MockOpenRouterHttpClient::new();
            let or_notifier = OrMockNotifier::new();
            let or_agent =
                OpenRouterAgent::with_deps(or_notifier, "test-model", "", or_http);

            // Create an empty openrouter session to import into
            or_agent.test_insert_session("or-s1").await;

            // ── 4. Import the xai export into openrouter ─────────────────────────
            let import_body = format!(
                r#"{{"sessionId":"or-s1","messages":{exported_json}}}"#
            );
            let import_params =
                serde_json::value::RawValue::from_string(import_body).unwrap();
            or_agent
                .ext_method(ExtRequest::new("session/import", Arc::from(import_params)))
                .await
                .expect("openrouter session/import should succeed");

            // ── 5. Export from openrouter ────────────────────────────────────────
            let or_export_params = serde_json::value::RawValue::from_string(
                serde_json::json!({ "sessionId": "or-s1" }).to_string(),
            )
            .unwrap();
            let or_export_resp = or_agent
                .ext_method(ExtRequest::new(
                    "session/export",
                    Arc::from(or_export_params),
                ))
                .await
                .expect("openrouter session/export should succeed");

            let or_portable: Vec<PortableMessage> =
                serde_json::from_str(or_export_resp.0.get())
                    .expect("openrouter export should be valid JSON");

            // ── 6. Assert round-trip fidelity ────────────────────────────────────
            assert_eq!(
                xai_portable.len(),
                or_portable.len(),
                "message count should match after cross-runner round-trip"
            );
            for (xai_msg, or_msg) in xai_portable.iter().zip(or_portable.iter()) {
                assert_eq!(
                    xai_msg.role, or_msg.role,
                    "role mismatch in cross-runner round-trip"
                );
                assert_eq!(
                    xai_msg.text, or_msg.text,
                    "text mismatch in cross-runner round-trip"
                );
            }
        })
        .await;
}

/// Export a session from acp-runner (backed by real NATS KV) and import it
/// into openrouter-runner. Verifies role/text fidelity across the acp→openrouter
/// boundary using a real JetStream store on the source side.
#[tokio::test]
async fn cross_runner_acp_export_into_openrouter_import() {
    let (_c, port) = start_nats_js().await;
    let (_, js) = make_js(port).await;

    local()
        .run_until(async move {
            // ── 1. Seed an acp session in NATS KV ────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let state = SessionState {
                messages: vec![
                    AgentMessage::user_text("question"),
                    AgentMessage::assistant(vec![AgentContentBlock::Text {
                        text: "answer".into(),
                    }]),
                ],
                ..Default::default()
            };
            store.save("acp-s1", &state).await.unwrap();
            let acp_agent = make_acp_agent(store);

            // ── 2. Export from acp ────────────────────────────────────────────────
            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "acp-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let acp_export_resp = acp_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("acp session/export should succeed");

            let exported_json = acp_export_resp.0.get().to_string();
            let acp_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("acp export should be valid JSON");
            assert_eq!(acp_portable.len(), 2);
            assert_eq!(acp_portable[0].role, "user");
            assert_eq!(acp_portable[0].text, "question");
            assert_eq!(acp_portable[1].role, "assistant");
            assert_eq!(acp_portable[1].text, "answer");

            // ── 3. Import into openrouter ─────────────────────────────────────────
            let or_http = MockOpenRouterHttpClient::new();
            let or_notifier = OrMockNotifier::new();
            let or_agent = OpenRouterAgent::with_deps(or_notifier, "test-model", "", or_http);
            or_agent.test_insert_session("or-s1").await;

            let import_body = format!(r#"{{"sessionId":"or-s1","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            or_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("openrouter session/import should succeed");

            // ── 4. Export from openrouter and verify fidelity ─────────────────────
            let or_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "or-s1" }).to_string(),
                )
                .unwrap()
                .into();
            let or_export_resp = or_agent
                .ext_method(ExtRequest::new("session/export", or_export_params))
                .await
                .expect("openrouter session/export should succeed");

            let or_portable: Vec<PortableMessage> =
                serde_json::from_str(or_export_resp.0.get())
                    .expect("openrouter export should be valid JSON");

            assert_eq!(
                acp_portable.len(),
                or_portable.len(),
                "message count should match after acp→openrouter round-trip"
            );
            for (acp_msg, or_msg) in acp_portable.iter().zip(or_portable.iter()) {
                assert_eq!(acp_msg.role, or_msg.role, "role mismatch in acp→openrouter round-trip");
                assert_eq!(acp_msg.text, or_msg.text, "text mismatch in acp→openrouter round-trip");
            }
        })
        .await;
}

/// Export a session from xai-runner and import it into acp-runner (backed by
/// real NATS KV). Verifies role/text fidelity across the xai→acp boundary
/// using a real JetStream store on the destination side.
#[tokio::test]
async fn cross_runner_xai_export_into_acp_import() {
    let (_c, port) = start_nats_js().await;
    let (_, js) = make_js(port).await;

    local()
        .run_until(async move {
            // ── 1. Seed a xai session and export it ───────────────────────────────
            let xai_http = Arc::new(MockXaiHttpClient::new());
            let xai_notifier = Arc::new(XaiMockNotifier::new());
            let xai_agent = XaiAgent::with_deps(xai_notifier, "grok-3", "test-key", xai_http);
            xai_agent
                .test_insert_session_with_history(
                    "xai-s2",
                    "/tmp",
                    vec![
                        XaiMessage::user("hello"),
                        XaiMessage::assistant_text("world"),
                    ],
                )
                .await;

            let export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "xai-s2" }).to_string(),
                )
                .unwrap()
                .into();
            let xai_export_resp = xai_agent
                .ext_method(ExtRequest::new("session/export", export_params))
                .await
                .expect("xai session/export should succeed");

            let exported_json = xai_export_resp.0.get().to_string();
            let xai_portable: Vec<PortableMessage> =
                serde_json::from_str(&exported_json).expect("xai export should be valid JSON");
            assert_eq!(xai_portable.len(), 2);

            // ── 2. Import into acp (real NATS KV) ────────────────────────────────
            let store = NatsSessionStore::open(&js).await.unwrap();
            let acp_agent = make_acp_agent(store);

            let import_body = format!(r#"{{"sessionId":"acp-s2","messages":{exported_json}}}"#);
            let import_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(import_body).unwrap().into();
            acp_agent
                .ext_method(ExtRequest::new("session/import", import_params))
                .await
                .expect("acp session/import should succeed");

            // ── 3. Export from acp and verify fidelity ────────────────────────────
            let acp_export_params: Arc<serde_json::value::RawValue> =
                serde_json::value::RawValue::from_string(
                    serde_json::json!({ "sessionId": "acp-s2" }).to_string(),
                )
                .unwrap()
                .into();
            let acp_export_resp = acp_agent
                .ext_method(ExtRequest::new("session/export", acp_export_params))
                .await
                .expect("acp session/export should succeed");

            let acp_portable: Vec<PortableMessage> =
                serde_json::from_str(acp_export_resp.0.get())
                    .expect("acp export should be valid JSON");

            assert_eq!(
                xai_portable.len(),
                acp_portable.len(),
                "message count should match after xai→acp round-trip"
            );
            for (xai_msg, acp_msg) in xai_portable.iter().zip(acp_portable.iter()) {
                assert_eq!(xai_msg.role, acp_msg.role, "role mismatch in xai→acp round-trip");
                assert_eq!(xai_msg.text, acp_msg.text, "text mismatch in xai→acp round-trip");
            }
        })
        .await;
}
