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

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::{AcpPrefix, AgentHandler};
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    LogoutRequest, LogoutResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionId, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse, StopReason,
};
use async_nats::jetstream;
use futures::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_nats::jetstream::NatsJetStreamClient;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Encodes `params` as an ADR-0011 JSON-RPC-over-NATS request (method + id in
/// headers, params in body) and sends it request-reply over core NATS.
async fn wire_request<P: serde::Serialize>(
    nats: &async_nats::Client,
    subject: &str,
    method: &str,
    params: &P,
) -> async_nats::Message {
    let encoded = jsonrpc_nats::encode(&jsonrpc_nats::Message::Request {
        id: jsonrpc_nats::RequestId::Number(1),
        method: method.to_string(),
        params: serde_json::to_value(params).unwrap(),
    })
    .unwrap();
    nats.request_with_headers(subject.to_string(), encoded.headers, encoded.body)
        .await
        .expect("request must succeed")
}

/// Decodes a JSON-RPC-over-NATS success reply and returns its `result` value,
/// panicking with the decoded message when the reply is not a success.
fn wire_success_result(reply: &async_nats::Message) -> serde_json::Value {
    let decoded = jsonrpc_nats::decode(
        jsonrpc_nats::Direction::Response,
        None,
        reply.headers.as_ref().expect("reply must carry headers"),
        &reply.payload,
    )
    .expect("reply must decode");
    match decoded {
        jsonrpc_nats::Message::Success { result, .. } => result,
        other => panic!("expected success reply, got: {other:?}"),
    }
}

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

#[derive(Default)]
struct MockAgent {
    prompt_called: Arc<Mutex<bool>>,
    cancel_called: Arc<Mutex<bool>>,
    ext_method_called: Arc<Mutex<bool>>,
    ext_notification_called: Arc<Mutex<bool>>,
}

#[async_trait::async_trait]
impl AgentHandler for MockAgent {
    async fn initialize(&self, _: InitializeRequest) -> agent_client_protocol::Result<InitializeResponse> {
        Ok(InitializeResponse::new(ProtocolVersion::V0))
    }

    async fn authenticate(&self, _: AuthenticateRequest) -> agent_client_protocol::Result<AuthenticateResponse> {
        Err(agent_client_protocol::Error::method_not_found())
    }

    async fn new_session(&self, _: NewSessionRequest) -> agent_client_protocol::Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("test-session"))
    }

    async fn prompt(&self, _args: PromptRequest) -> agent_client_protocol::Result<PromptResponse> {
        *self.prompt_called.lock().unwrap() = true;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _: CancelNotification) -> agent_client_protocol::Result<()> {
        *self.cancel_called.lock().unwrap() = true;
        Ok(())
    }

    async fn ext_method(&self, _: ExtRequest) -> agent_client_protocol::Result<ExtResponse> {
        *self.ext_method_called.lock().unwrap() = true;
        Ok(ExtResponse::new(
            serde_json::value::RawValue::from_string(r#"{"ok":true}"#.to_string())
                .unwrap()
                .into(),
        ))
    }

    async fn ext_notification(&self, _: ExtNotification) -> agent_client_protocol::Result<()> {
        *self.ext_notification_called.lock().unwrap() = true;
        Ok(())
    }

    async fn logout(&self, _: LogoutRequest) -> agent_client_protocol::Result<LogoutResponse> {
        Ok(LogoutResponse::new())
    }

    async fn list_sessions(&self, _: ListSessionsRequest) -> agent_client_protocol::Result<ListSessionsResponse> {
        Ok(ListSessionsResponse::new(vec![]))
    }

    async fn load_session(&self, _: LoadSessionRequest) -> agent_client_protocol::Result<LoadSessionResponse> {
        Ok(LoadSessionResponse::new())
    }

    async fn set_session_mode(
        &self,
        _: SetSessionModeRequest,
    ) -> agent_client_protocol::Result<SetSessionModeResponse> {
        Ok(SetSessionModeResponse::new())
    }

    async fn set_session_config_option(
        &self,
        _: SetSessionConfigOptionRequest,
    ) -> agent_client_protocol::Result<SetSessionConfigOptionResponse> {
        Ok(SetSessionConfigOptionResponse::new(vec![]))
    }

    async fn fork_session(&self, _: ForkSessionRequest) -> agent_client_protocol::Result<ForkSessionResponse> {
        Ok(ForkSessionResponse::new(SessionId::from("forked-in-serve-js")))
    }

    async fn resume_session(&self, _: ResumeSessionRequest) -> agent_client_protocol::Result<ResumeSessionResponse> {
        Ok(ResumeSessionResponse::new())
    }

    async fn close_session(&self, _: CloseSessionRequest) -> agent_client_protocol::Result<CloseSessionResponse> {
        Ok(CloseSessionResponse::new())
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
        ..Default::default()
    };

    // Subscribe to the response subject BEFORE starting io_task so no message is missed.
    let response_subject = format!("acp.session.{}.agent.prompt.response.{}", session_id, req_id);
    let mut response_sub = nats.subscribe(response_subject).await.unwrap();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats.clone(), js_client, prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(io_task);

            // Let serve_js create its COMMANDS consumer before we publish.
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Publish the prompt command to the COMMANDS stream. Both the jsonrpc
            // `HEADER_ID` (required by `decode_request_params`) and the legacy
            // `X-Req-Id` header (used by serve_js for reply-subject routing) must
            // be present.
            let encoded = jsonrpc_nats::encode(&jsonrpc_nats::Message::Request {
                id: jsonrpc_nats::RequestId::Number(1),
                method: "prompt".to_string(),
                params: serde_json::to_value(PromptRequest::new(session_id, vec![])).unwrap(),
            })
            .unwrap();
            let mut headers = encoded.headers;
            headers.insert("X-Req-Id", req_id);
            js_ctx
                .publish_with_headers(
                    format!("acp.session.{}.agent.prompt", session_id),
                    headers,
                    encoded.body,
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

            let response: PromptResponse = serve_js_success(&msg);
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

    let agent = MockAgent::default();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats.clone(), js_client, prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(io_task);

            // Let serve_global subscribe to the global wildcard before sending.
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Send a core-NATS request — serve_global handles it via the acp.agent.> sub.
            let req = InitializeRequest::new(ProtocolVersion::V0);
            let reply = tokio::time::timeout(
                Duration::from_secs(5),
                wire_request(&nats, "acp.agent.initialize", "initialize", &req),
            )
            .await
            .expect("timed out waiting for initialize response");

            let resp: InitializeResponse =
                serde_json::from_value(wire_success_result(&reply)).expect("invalid InitializeResponse JSON");
            assert_eq!(
                resp.protocol_version,
                ProtocolVersion::V0,
                "serve_global must relay the agent's initialize response"
            );
        })
        .await;
}

/// `serve_global` (run by `with_jetstream`) also subscribes to
/// `{prefix}.session.*.agent.ext.>` (AllAgentExtSubject).  When a request
/// arrives there it is parsed as a global Ext method and dispatched to the
/// agent's `ext_method()` handler; the response is published to the reply inbox.
#[tokio::test]
async fn serve_global_dispatches_session_ext_method_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let ext_method_called = Arc::new(Mutex::new(false));
    let agent = MockAgent {
        ext_method_called: ext_method_called.clone(),
        ..Default::default()
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats.clone(), js_client, prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(io_task);

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Session-scoped ext subject is caught by AllAgentExtSubject subscription
            // and parsed as GlobalAgentMethod::Ext — serve_global calls ext_method().
            let req = ExtRequest::new(
                "my_op",
                serde_json::value::RawValue::from_string("{}".to_string())
                    .unwrap()
                    .into(),
            );
            let reply = tokio::time::timeout(
                Duration::from_secs(5),
                wire_request(&nats, "acp.session.sess-ext.agent.ext.my_op", "ext", &req),
            )
            .await
            .expect("timed out waiting for ext_method response");

            let _resp: ExtResponse =
                serde_json::from_value(wire_success_result(&reply)).expect("invalid ExtResponse JSON");
        })
        .await;

    assert!(
        *ext_method_called.lock().unwrap(),
        "agent.ext_method() must have been called via session ext subscription"
    );
}

/// `serve_global` dispatches ext publishes that carry no reply subject to the
/// agent's `ext_notification()` handler (fire-and-forget).
#[tokio::test]
async fn serve_global_dispatches_ext_notification_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let ext_notification_called = Arc::new(Mutex::new(false));
    let agent = MockAgent {
        ext_notification_called: ext_notification_called.clone(),
        ..Default::default()
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats.clone(), js_client, prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(io_task);

            tokio::time::sleep(Duration::from_millis(100)).await;

            // No reply subject → serve_global calls ext_notification() instead of ext_method().
            let payload = serde_json::to_vec(&ExtNotification::new(
                "my_notify",
                serde_json::value::RawValue::from_string("{}".to_string())
                    .unwrap()
                    .into(),
            ))
            .unwrap();
            nats.publish("acp.agent.ext.my_notify", payload.into()).await.unwrap();

            // Give the dispatch loop time to call the handler.
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;

    assert!(
        *ext_notification_called.lock().unwrap(),
        "agent.ext_notification() must have been called for fire-and-forget ext publish"
    );
}

/// `serve_js` receives a cancel message from the JetStream COMMANDS stream.
/// Cancel is a notification — no `X-Req-Id`, no response published — but the
/// agent's `cancel()` handler must still be called and the message acked.
#[tokio::test]
async fn serve_js_dispatches_cancel_notification_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "cancel-sess";
    let cancel_called = Arc::new(Mutex::new(false));
    let agent = MockAgent {
        cancel_called: cancel_called.clone(),
        ..Default::default()
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats.clone(), js_client, prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(io_task);

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Cancel arrives via JetStream with no X-Req-Id — it is a notification.
            let payload = serde_json::to_vec(&CancelNotification::new(session_id)).unwrap();
            js_ctx
                .publish(format!("acp.session.{}.agent.cancel", session_id), payload.into())
                .await
                .unwrap()
                .await
                .unwrap();

            // Give serve_js time to dispatch the notification.
            tokio::time::sleep(Duration::from_millis(500)).await;
        })
        .await;

    assert!(
        *cancel_called.lock().unwrap(),
        "agent.cancel() must have been called via JetStream cancel notification"
    );
}

// ── helper ────────────────────────────────────────────────────────────────────

/// Start `AgentSideNatsConnection::with_jetstream` for the given agent, wait
/// 100 ms for subscriptions to be established, then return the LocalSet future
/// handle and a function to run the inner test logic.
///
/// All serve_global + serve_js tests share the same boilerplate; this macro-like
/// helper keeps them short.
async fn run_with_jetstream<A: AgentHandler + Sync + 'static>(
    nats: async_nats::Client,
    js_ctx: async_nats::jetstream::Context,
    agent: A,
    inner: impl std::future::Future<Output = ()>,
) {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx);
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) = AgentSideNatsConnection::with_jetstream(agent, nats, js_client, prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            tokio::task::spawn_local(io_task);
            tokio::time::sleep(Duration::from_millis(100)).await;
            inner.await;
        })
        .await;
}

// ── serve_global: authenticate ────────────────────────────────────────────────

#[tokio::test]
async fn serve_global_dispatches_authenticate_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let nats2 = nats.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = AuthenticateRequest::new("secret");
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            wire_request(&nats2, "acp.agent.authenticate", "authenticate", &req),
        )
        .await
        .expect("timed out");

        // MockAgent.authenticate() returns method_not_found; verify serve_global
        // relayed a JSON-RPC error reply rather than dropping the request.
        let decoded = jsonrpc_nats::decode(
            jsonrpc_nats::Direction::Response,
            None,
            reply.headers.as_ref().expect("reply must carry headers"),
            &reply.payload,
        )
        .expect("reply must decode");
        assert!(
            matches!(decoded, jsonrpc_nats::Message::Error { .. }),
            "serve_global must relay the agent's method_not_found error, got: {decoded:?}"
        );
    })
    .await;
}

// ── serve_global: logout ──────────────────────────────────────────────────────

#[tokio::test]
async fn serve_global_dispatches_logout_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let nats2 = nats.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = LogoutRequest::new();
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            wire_request(&nats2, "acp.agent.logout", "logout", &req),
        )
        .await
        .expect("timed out");

        let _resp: LogoutResponse =
            serde_json::from_value(wire_success_result(&reply)).expect("invalid LogoutResponse JSON");
    })
    .await;
}

// ── serve_global: new_session ─────────────────────────────────────────────────

#[tokio::test]
async fn serve_global_dispatches_new_session_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let nats2 = nats.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = NewSessionRequest::new(".");
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            wire_request(&nats2, "acp.agent.session.new", "session/new", &req),
        )
        .await
        .expect("timed out");

        let resp: NewSessionResponse =
            serde_json::from_value(wire_success_result(&reply)).expect("invalid NewSessionResponse JSON");
        assert_eq!(
            resp.session_id.to_string().as_str(),
            "test-session",
            "serve_global must relay the agent's new_session response"
        );
    })
    .await;
}

// ── serve_global: list_sessions ───────────────────────────────────────────────

#[tokio::test]
async fn serve_global_dispatches_list_sessions_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let nats2 = nats.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = ListSessionsRequest::new();
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            wire_request(&nats2, "acp.agent.session.list", "session/list", &req),
        )
        .await
        .expect("timed out");

        let _resp: ListSessionsResponse =
            serde_json::from_value(wire_success_result(&reply)).expect("invalid ListSessionsResponse JSON");
    })
    .await;
}

// ── serve_js: helpers ─────────────────────────────────────────────────────────

/// Publish a JSON-RPC-over-NATS command carrying BOTH the jsonrpc `HEADER_ID`
/// (required by `decode_request_params`) and the legacy `X-Req-Id` header
/// (used by `serve_js` for reply-subject routing), then wait for the response
/// that `serve_js` publishes to core NATS.
async fn serve_js_round_trip<P: serde::Serialize>(
    nats: &async_nats::Client,
    js_ctx: &async_nats::jetstream::Context,
    subject: &str,
    method: &str,
    req_id: &str,
    params: &P,
    response_subject: &str,
) -> async_nats::Message {
    let mut sub = nats.subscribe(response_subject.to_string()).await.unwrap();

    let encoded = jsonrpc_nats::encode(&jsonrpc_nats::Message::Request {
        id: jsonrpc_nats::RequestId::Number(1),
        method: method.to_string(),
        params: serde_json::to_value(params).unwrap(),
    })
    .unwrap();
    let mut headers = encoded.headers;
    headers.insert("X-Req-Id", req_id);
    js_ctx
        .publish_with_headers(subject.to_string(), headers, encoded.body)
        .await
        .unwrap()
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for serve_js response")
        .expect("response subscription closed")
}

/// Decodes a `serve_js` JSON-RPC-over-NATS success reply and deserializes its
/// `result` value into `T`.
fn serve_js_success<T: serde::de::DeserializeOwned>(msg: &async_nats::Message) -> T {
    let decoded = jsonrpc_nats::decode(
        jsonrpc_nats::Direction::Response,
        None,
        msg.headers.as_ref().expect("reply must carry headers"),
        &msg.payload,
    )
    .expect("reply must decode");
    match decoded {
        jsonrpc_nats::Message::Success { result, .. } => {
            serde_json::from_value(result).expect("result must deserialize")
        }
        other => panic!("expected success reply, got: {other:?}"),
    }
}

// ── serve_js: load_session ────────────────────────────────────────────────────

#[tokio::test]
async fn serve_js_dispatches_load_session_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "load-js-sess";
    let req_id = "req-load-1";
    let response_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);

    let nats2 = nats.clone();
    let js2 = js_ctx.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = LoadSessionRequest::new(session_id, ".");
        let msg = serve_js_round_trip(
            &nats2,
            &js2,
            &format!("acp.session.{}.agent.load", session_id),
            "session/load",
            req_id,
            &req,
            &response_subject,
        )
        .await;
        let _resp: LoadSessionResponse = serve_js_success(&msg);
    })
    .await;
}

// ── serve_js: set_session_mode ────────────────────────────────────────────────

#[tokio::test]
async fn serve_js_dispatches_set_session_mode_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "set-mode-js-sess";
    let req_id = "req-mode-1";
    let response_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);

    let nats2 = nats.clone();
    let js2 = js_ctx.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = SetSessionModeRequest::new(session_id, "edit");
        let msg = serve_js_round_trip(
            &nats2,
            &js2,
            &format!("acp.session.{}.agent.set_mode", session_id),
            "session/set_mode",
            req_id,
            &req,
            &response_subject,
        )
        .await;
        let _resp: SetSessionModeResponse = serve_js_success(&msg);
    })
    .await;
}

// ── serve_js: set_session_config_option ───────────────────────────────────────

#[tokio::test]
async fn serve_js_dispatches_set_session_config_option_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "cfg-opt-js-sess";
    let req_id = "req-cfg-1";
    let response_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);

    let nats2 = nats.clone();
    let js2 = js_ctx.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = SetSessionConfigOptionRequest::new(session_id, "theme", "dark");
        let msg = serve_js_round_trip(
            &nats2,
            &js2,
            &format!("acp.session.{}.agent.set_config_option", session_id),
            "session/set_config_option",
            req_id,
            &req,
            &response_subject,
        )
        .await;
        let _resp: SetSessionConfigOptionResponse = serve_js_success(&msg);
    })
    .await;
}

// ── serve_js: fork_session ────────────────────────────────────────────────────

#[tokio::test]
async fn serve_js_dispatches_fork_session_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "fork-js-sess";
    let req_id = "req-fork-1";
    let response_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);

    let nats2 = nats.clone();
    let js2 = js_ctx.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = ForkSessionRequest::new(session_id, ".");
        let msg = serve_js_round_trip(
            &nats2,
            &js2,
            &format!("acp.session.{}.agent.fork", session_id),
            "session/fork",
            req_id,
            &req,
            &response_subject,
        )
        .await;
        let resp: ForkSessionResponse = serve_js_success(&msg);
        assert_eq!(
            resp.session_id.to_string().as_str(),
            "forked-in-serve-js",
            "serve_js must relay the agent's fork_session response"
        );
    })
    .await;
}

// ── serve_js: resume_session ──────────────────────────────────────────────────

#[tokio::test]
async fn serve_js_dispatches_resume_session_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "resume-js-sess";
    let req_id = "req-resume-1";
    let response_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);

    let nats2 = nats.clone();
    let js2 = js_ctx.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = ResumeSessionRequest::new(session_id, ".");
        let msg = serve_js_round_trip(
            &nats2,
            &js2,
            &format!("acp.session.{}.agent.resume", session_id),
            "session/resume",
            req_id,
            &req,
            &response_subject,
        )
        .await;
        let _resp: ResumeSessionResponse = serve_js_success(&msg);
    })
    .await;
}

// ── serve_js: DeliverNew — stale messages are ignored ────────────────────────

/// `commands_observer` uses `DeliverPolicy::New`, so messages published to the
/// COMMANDS stream BEFORE the consumer is created must NOT be dispatched to the
/// agent.  Messages published AFTER consumer creation must still be dispatched.
#[tokio::test]
async fn serve_js_deliver_new_ignores_pre_existing_messages() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "deliver-new-sess";

    // Publish 3 prompt commands into the stream BEFORE any consumer exists.
    for i in 0..3_u32 {
        let req_id = format!("stale-req-{i}");
        let encoded = jsonrpc_nats::encode(&jsonrpc_nats::Message::Request {
            id: jsonrpc_nats::RequestId::Number(1),
            method: "prompt".to_string(),
            params: serde_json::to_value(PromptRequest::new(session_id, vec![])).unwrap(),
        })
        .unwrap();
        let mut headers = encoded.headers;
        headers.insert("X-Req-Id", req_id.as_str());
        js_ctx
            .publish_with_headers(format!("acp.session.{session_id}.agent.prompt"), headers, encoded.body)
            .await
            .unwrap()
            .await
            .unwrap();
    }

    let prompt_called = Arc::new(Mutex::new(false));
    let agent = MockAgent {
        prompt_called: prompt_called.clone(),
        ..Default::default()
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let js_client = NatsJetStreamClient::new(js_ctx.clone());
            let prefix = AcpPrefix::new("acp").unwrap();
            let (_conn, io_task) =
                AgentSideNatsConnection::with_jetstream(agent, nats.clone(), js_client, prefix, |fut| {
                    tokio::task::spawn_local(fut);
                });
            tokio::task::spawn_local(io_task);

            // Give the consumer enough time to process any stale messages if DeliverAll.
            tokio::time::sleep(Duration::from_millis(500)).await;

            assert!(
                !*prompt_called.lock().unwrap(),
                "DeliverNew: pre-existing stream messages must NOT trigger agent.prompt()"
            );
        })
        .await;
}

/// After `AgentSideNatsConnection::with_jetstream` starts its consumer
/// (`DeliverPolicy::New`), messages published from that point on ARE dispatched.
#[tokio::test]
async fn serve_js_deliver_new_receives_messages_published_after_consumer_creation() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "deliver-new-fresh-sess";
    let req_id = "req-after-start";
    let response_subject = format!("acp.session.{}.agent.prompt.response.{}", session_id, req_id);

    let prompt_called = Arc::new(Mutex::new(false));
    let agent = MockAgent {
        prompt_called: prompt_called.clone(),
        ..Default::default()
    };

    let nats2 = nats.clone();
    let js2 = js_ctx.clone();
    run_with_jetstream(nats, js_ctx, agent, async move {
        // Consumer is now active.  Publish one fresh message.
        let req = PromptRequest::new(session_id, vec![]);
        let msg = serve_js_round_trip(
            &nats2,
            &js2,
            &format!("acp.session.{session_id}.agent.prompt"),
            "prompt",
            req_id,
            &req,
            &response_subject,
        )
        .await;
        let resp: PromptResponse = serve_js_success(&msg);
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    })
    .await;

    assert!(
        *prompt_called.lock().unwrap(),
        "DeliverNew: message published after consumer creation must be dispatched"
    );
}

// ── serve_js: close_session ───────────────────────────────────────────────────

#[tokio::test]
async fn serve_js_dispatches_close_session_to_agent() {
    let (_c, port) = start_nats().await;
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js_ctx = jetstream::new(nats.clone());
    setup_streams(&js_ctx).await;

    let session_id = "close-js-sess";
    let req_id = "req-close-1";
    let response_subject = format!("acp.session.{}.agent.response.{}", session_id, req_id);

    let nats2 = nats.clone();
    let js2 = js_ctx.clone();
    run_with_jetstream(nats, js_ctx, MockAgent::default(), async move {
        let req = CloseSessionRequest::new(session_id);
        let msg = serve_js_round_trip(
            &nats2,
            &js2,
            &format!("acp.session.{}.agent.close", session_id),
            "session/close",
            req_id,
            &req,
            &response_subject,
        )
        .await;
        let _resp: CloseSessionResponse = serve_js_success(&msg);
    })
    .await;
}
