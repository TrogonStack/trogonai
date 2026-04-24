//! Integration test for `AgentSideNatsConnection::with_jetstream` against a
//! real NATS JetStream server.
//!
//! Verifies that `serve_js` creates a consumer on the COMMANDS stream, dispatches
//! an incoming prompt command to the agent's `prompt()` handler, and publishes
//! the response to the correct RESPONSES subject on core NATS.
//!
//! Requires Docker (uses testcontainers to spin up a NATS JetStream server).
//!
//! Run with:
//!   cargo test -p acp-nats-agent --test serve_js_integration

use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ProtocolVersion, StopReason,
};
use async_nats::jetstream;
use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_nats::jetstream::NatsJetStreamClient;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["-js"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn setup_streams(js: &jetstream::Context) {
    let prefix = AcpPrefix::new("acp").unwrap();
    for config in acp_nats::jetstream::streams::all_configs(&prefix) {
        js.get_or_create_stream(config).await.unwrap();
    }
}

// ── MockAgent ─────────────────────────────────────────────────────────────────

struct MockAgent {
    prompt_called: Arc<Mutex<bool>>,
}

#[async_trait::async_trait(?Send)]
impl Agent for MockAgent {
    async fn initialize(
        &self,
        _: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        Ok(InitializeResponse::new(
            agent_client_protocol::ProtocolVersion::V0,
        ))
    }

    async fn authenticate(
        &self,
        _: AuthenticateRequest,
    ) -> agent_client_protocol::Result<AuthenticateResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn new_session(
        &self,
        _: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("test-session"))
    }

    async fn prompt(
        &self,
        _args: PromptRequest,
    ) -> agent_client_protocol::Result<PromptResponse> {
        *self.prompt_called.lock().unwrap() = true;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _: CancelNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}

// ── test ─────────────────────────────────────────────────────────────────────

/// `serve_js` subscribes to the COMMANDS JetStream stream. When a prompt
/// command arrives (with an `X-Req-Id` header), it dispatches to the agent's
/// `prompt()` handler and publishes the response to core NATS on the subject
/// `{prefix}.session.{id}.agent.prompt.response.{req_id}`.
#[tokio::test]
async fn serve_js_dispatches_prompt_command_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "serve-js-sess";
    let req_id = "req-serve-js-1";

    let prompt_called = Arc::new(Mutex::new(false));
    let agent = MockAgent {
        prompt_called: prompt_called.clone(),
    };

    // Subscribe to the response subject BEFORE starting io_task so no message is missed.
    let response_subject = format!("acp.session.{}.agent.prompt.response.{}", session_id, req_id);
    let mut response_sub = nats.subscribe(response_subject).await.unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) = AgentSideNatsConnection::with_jetstream(
                agent,
                nats.clone(),
                js_client,
                prefix,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(io_task);

            // Let serve_js create its COMMANDS consumer before we publish.
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Publish the prompt command to the COMMANDS stream.
            let mut headers = async_nats::HeaderMap::new();
            headers.insert("X-Req-Id", req_id);
            let payload =
                serde_json::to_vec(&PromptRequest::new(session_id, vec![])).unwrap();
            js_ctx
                .publish_with_headers(
                    format!("acp.session.{}.agent.prompt", session_id),
                    headers,
                    payload.into(),
                )
                .await
                .unwrap()
                .await
                .unwrap();

            // Wait for the response that serve_js publishes to core NATS.
            let msg = tokio::time::timeout(Duration::from_secs(5), response_sub.next())
                .await
                .expect("timed out waiting for prompt response")
                .expect("response subscription closed unexpectedly");

            let response: PromptResponse =
                serde_json::from_slice(&msg.payload).expect("invalid PromptResponse JSON");
            assert_eq!(
                response.stop_reason,
                StopReason::EndTurn,
                "response must carry StopReason::EndTurn"
            );
        })
        .await;

    assert!(
        *prompt_called.lock().unwrap(),
        "agent.prompt() must have been called by serve_js"
    );
}

/// `serve_global` (run by `with_jetstream`) subscribes to the global NATS wildcard
/// `{prefix}.agent.>`. When a core-NATS request arrives on `{prefix}.agent.initialize`,
/// it dispatches to the agent's `initialize()` handler and replies on the reply inbox.
#[tokio::test]
async fn serve_global_dispatches_initialize_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let agent = MockAgent {
        prompt_called: Arc::new(Mutex::new(false)),
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) = AgentSideNatsConnection::with_jetstream(
                agent,
                nats.clone(),
                js_client,
                prefix,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(io_task);

            // Let serve_global subscribe to the global wildcard before sending.
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Send a core-NATS request — serve_global handles it via the acp.agent.> sub.
            let req = InitializeRequest::new(ProtocolVersion::V0);
            let payload = serde_json::to_vec(&req).unwrap();
            let msg = tokio::time::timeout(
                Duration::from_secs(5),
                nats.request("acp.agent.initialize", payload.into()),
            )
            .await
            .expect("timed out waiting for initialize response")
            .expect("request failed");

            let resp: InitializeResponse =
                serde_json::from_slice(&msg.payload).expect("invalid InitializeResponse JSON");
            assert_eq!(
                resp.protocol_version,
                ProtocolVersion::V0,
                "serve_global must relay the agent's initialize response"
            );
        })
        .await;
}
