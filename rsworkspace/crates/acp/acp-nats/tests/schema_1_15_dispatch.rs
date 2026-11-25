#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use acp_nats::AgentHandler;
use acp_nats::boundary::{BoundaryExit, connect_agent_boundary};
use agent_client_protocol::Result;
use agent_client_protocol::v1::schema::{
    ArchiveSessionRequest, ArchiveSessionResponse, AuthStatusRequest, AuthStatusResponse, AuthenticateRequest,
    AuthenticateResponse, CancelNotification, DisableProviderRequest, DisableProviderResponse, InitializeRequest,
    InitializeResponse, ListProvidersRequest, ListProvidersResponse, NewSessionRequest, NewSessionResponse,
    PromptRequest, PromptResponse, SessionStatusRequest, SessionStatusResponse, SetProviderRequest,
    SetProviderResponse, SetSessionTitleRequest, SetSessionTitleResponse, StopReason, UnarchiveSessionRequest,
    UnarchiveSessionResponse,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines};

#[derive(Debug, PartialEq)]
struct Call {
    method: &'static str,
    params: Value,
}

#[derive(Default)]
struct RecordingAgent {
    calls: Mutex<Vec<Call>>,
    initializations: AtomicUsize,
    cancellations: AtomicUsize,
}

impl RecordingAgent {
    fn respond<T: Serialize, U: DeserializeOwned>(&self, method: &'static str, params: T) -> Result<U> {
        self.calls.lock().unwrap().push(Call {
            method,
            params: serde_json::to_value(params).unwrap(),
        });
        Ok(serde_json::from_value(response_fixture(method)).unwrap())
    }
}

#[async_trait::async_trait]
impl AgentHandler for RecordingAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.initializations.fetch_add(1, Ordering::SeqCst);
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        self.calls.lock().unwrap().push(Call {
            method: "session/new",
            params: serde_json::to_value(args).unwrap(),
        });
        Ok(NewSessionResponse::new("session-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        self.cancellations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn auth_status(&self, args: AuthStatusRequest) -> Result<AuthStatusResponse> {
        self.respond("auth/status", args)
    }

    async fn session_status(&self, args: SessionStatusRequest) -> Result<SessionStatusResponse> {
        self.respond("session/status", args)
    }

    async fn set_session_title(&self, args: SetSessionTitleRequest) -> Result<SetSessionTitleResponse> {
        self.respond("session/set_title", args)
    }

    async fn archive_session(&self, args: ArchiveSessionRequest) -> Result<ArchiveSessionResponse> {
        self.respond("session/archive", args)
    }

    async fn unarchive_session(&self, args: UnarchiveSessionRequest) -> Result<UnarchiveSessionResponse> {
        self.respond("session/unarchive", args)
    }

    async fn list_providers(&self, args: ListProvidersRequest) -> Result<ListProvidersResponse> {
        self.respond("providers/list", args)
    }

    async fn set_provider(&self, args: SetProviderRequest) -> Result<SetProviderResponse> {
        self.respond("providers/set", args)
    }

    async fn disable_provider(&self, args: DisableProviderRequest) -> Result<DisableProviderResponse> {
        self.respond("providers/disable", args)
    }
}

fn response_fixture(method: &str) -> Value {
    let mut response = match method {
        "auth/status" => json!({
            "state": "authenticated",
            "accounts": [{ "label": "Test account", "active": true }]
        }),
        "session/status" => json!({ "status": "open" }),
        "session/set_title" => json!({ "title": "Normalized title" }),
        "providers/list" => json!({
            "providers": [{
                "providerId": "provider-1",
                "supported": ["openai"],
                "required": false
            }]
        }),
        "session/archive" | "session/unarchive" | "providers/set" | "providers/disable" => json!({}),
        other => panic!("unexpected method {other}"),
    };
    response["_meta"] = json!({ "handled": method });
    response
}

fn method_fixtures() -> Vec<Call> {
    vec![
        Call {
            method: "auth/status",
            params: json!({ "_meta": { "request": "auth-status" } }),
        },
        Call {
            method: "session/status",
            params: json!({ "sessionId": "session-1", "_meta": { "request": "session-status" } }),
        },
        Call {
            method: "session/set_title",
            params: json!({
                "sessionId": "session-1",
                "title": "  Requested title  ",
                "_meta": { "request": "set-title" }
            }),
        },
        Call {
            method: "session/archive",
            params: json!({ "sessionId": "session-1", "_meta": { "request": "archive" } }),
        },
        Call {
            method: "session/unarchive",
            params: json!({ "sessionId": "session-1", "_meta": { "request": "unarchive" } }),
        },
        Call {
            method: "providers/list",
            params: json!({ "_meta": { "request": "list-providers" } }),
        },
        Call {
            method: "providers/set",
            params: json!({
                "providerId": "provider-1",
                "apiType": "openai",
                "baseUrl": "https://provider.example/v1",
                "headers": { "x-routing-label": "test-route" },
                "_meta": { "request": "set-provider" }
            }),
        },
        Call {
            method: "providers/disable",
            params: json!({ "providerId": "provider-1", "_meta": { "request": "disable-provider" } }),
        },
    ]
}

async fn send_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Value) {
    let mut bytes = serde_json::to_vec(frame).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
}

async fn read_frame<R: AsyncBufRead + Unpin>(lines: &mut Lines<R>) -> Value {
    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn new_methods_reach_their_handlers_and_return_the_typed_response() {
    let agent = Arc::new(RecordingAgent::default());
    let (client_io, server_io) = tokio::io::duplex(8192);
    let (client_read, mut client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);
    let boundary = connect_agent_boundary(
        agent.clone(),
        async_compat::Compat::new(server_write),
        async_compat::Compat::new(server_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    );

    let client = async move {
        let mut lines = BufReader::new(client_read).lines();
        send_frame(
            &mut client_write,
            &json!({
                "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": 1 }
            }),
        )
        .await;
        let initialized = read_frame(&mut lines).await;
        assert_eq!(initialized["id"], 0);
        assert_eq!(initialized["result"]["protocolVersion"], 1);

        for (index, call) in method_fixtures().into_iter().enumerate() {
            let id = index + 1;
            send_frame(
                &mut client_write,
                &json!({
                    "jsonrpc": "2.0", "id": id, "method": call.method, "params": call.params
                }),
            )
            .await;
            let response = read_frame(&mut lines).await;
            assert_eq!(response["id"], id, "{} response correlation", call.method);
            assert_eq!(
                response["result"],
                response_fixture(call.method),
                "{} response",
                call.method
            );
        }
        drop(client_write);
    };

    let (outcome, ()) = tokio::join!(boundary, client);
    assert!(matches!(outcome, Ok(BoundaryExit::TransportClosed)));
    assert_eq!(*agent.calls.lock().unwrap(), method_fixtures());
    assert_eq!(agent.initializations.load(Ordering::SeqCst), 1);
    assert_eq!(agent.cancellations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_jsonrpc_array_dispatches_no_entries_and_the_connection_accepts_the_next_request() {
    let agent = Arc::new(RecordingAgent::default());
    let (client_io, server_io) = tokio::io::duplex(8192);
    let (client_read, mut client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);
    let boundary = connect_agent_boundary(
        agent.clone(),
        async_compat::Compat::new(server_write),
        async_compat::Compat::new(server_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    );
    let observed = agent.clone();

    let client = async move {
        let mut lines = BufReader::new(client_read).lines();
        send_frame(&mut client_write, &json!([
            { "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": 1 } },
            { "jsonrpc": "2.0", "id": 2, "method": "session/new", "params": { "cwd": "/workspace", "mcpServers": [] } },
            { "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": "session-1" } },
            { "jsonrpc": "2.0", "id": 3, "method": "providers/list", "params": {} }
        ])).await;
        let rejected = read_frame(&mut lines).await;
        assert!(rejected.is_object());
        assert!(rejected.get("id").unwrap().is_null());
        assert_eq!(rejected["error"]["code"], -32600);
        assert!(rejected.get("result").is_none());

        send_frame(
            &mut client_write,
            &json!({
                "jsonrpc": "2.0", "id": 4, "method": "initialize", "params": { "protocolVersion": 1 }
            }),
        )
        .await;
        let initialized = read_frame(&mut lines).await;
        assert_eq!(initialized["id"], 4);
        assert_eq!(initialized["result"]["protocolVersion"], 1);
        assert_eq!(observed.initializations.load(Ordering::SeqCst), 1);
        assert_eq!(observed.cancellations.load(Ordering::SeqCst), 0);
        assert!(observed.calls.lock().unwrap().is_empty());
        drop(client_write);
    };

    let (outcome, ()) = tokio::join!(boundary, client);
    assert!(matches!(outcome, Ok(BoundaryExit::TransportClosed)));
    assert_eq!(agent.initializations.load(Ordering::SeqCst), 1);
    assert_eq!(agent.cancellations.load(Ordering::SeqCst), 0);
    assert!(agent.calls.lock().unwrap().is_empty());
}
