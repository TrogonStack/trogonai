use super::super::connection_client::ConnectionClient;
use super::*;
use crate::client_handler::ClientHandler;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CompleteElicitationNotification, ContentBlock, ContentChunk, CreateElicitationRequest, CreateElicitationResponse,
    CreateTerminalRequest, CreateTerminalResponse, DeleteSessionRequest, ElicitationAcceptAction, ElicitationFormMode,
    ElicitationSchema, ElicitationSessionScope, ExtNotification, ExtRequest, ForkSessionRequest, InitializeRequest,
    InitializeResponse, KillTerminalRequest, KillTerminalResponse, ListSessionsRequest, LoadSessionRequest,
    LogoutRequest, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest, SessionConfigOptionValue,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason,
    TerminalExitStatus, TerminalOutputRequest, TerminalOutputResponse, ToolCallUpdate, ToolCallUpdateFields,
    WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct StubAgent;

#[async_trait::async_trait]
impl AgentHandler for StubAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

struct PendingRead;

impl futures::AsyncRead for PendingRead {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Pending
    }
}

#[tokio::test]
async fn transport_eof_ends_connection_with_transport_closed() {
    let result = connect_agent_boundary(
        Arc::new(StubAgent),
        futures::io::sink(),
        futures::io::empty(),
        async move |_cx| std::future::pending::<Result<()>>().await,
    )
    .await;

    assert!(matches!(result, Ok(BoundaryExit::TransportClosed)));
}

#[tokio::test]
async fn main_fn_return_wins_over_open_transport() {
    let result = connect_agent_boundary(
        Arc::new(StubAgent),
        futures::io::sink(),
        PendingRead,
        async move |_cx| Ok(42u32),
    )
    .await;

    assert!(matches!(result, Ok(BoundaryExit::Main(42))));
}

#[tokio::test]
async fn initialize_round_trips_through_boundary() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let boundary = connect_agent_boundary(
        Arc::new(StubAgent),
        async_compat::Compat::new(server_write),
        async_compat::Compat::new(server_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    );

    let client = async move {
        let mut writer = client_write;
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n")
            .await
            .unwrap();
        let mut lines = BufReader::new(client_read).lines();
        let line = lines.next_line().await.unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 1);
        assert_eq!(value["result"]["protocolVersion"], 1);
        drop(writer);
    };

    let (boundary_result, ()) = tokio::join!(boundary, client);
    assert!(matches!(boundary_result, Ok(BoundaryExit::TransportClosed)));
}

struct PendingPromptAgent;

#[async_trait::async_trait]
impl AgentHandler for PendingPromptAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        std::future::pending().await
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn cancel_request_answers_pending_prompt_with_request_cancelled_error() {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let boundary = connect_agent_boundary(
        Arc::new(PendingPromptAgent),
        async_compat::Compat::new(server_write),
        async_compat::Compat::new(server_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    );

    let client = async move {
        let mut writer = client_write;
        writer
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"sess-1\",\"prompt\":[]}}\n",
            )
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"$/cancel_request\",\"params\":{\"requestId\":1}}\n")
            .await
            .unwrap();

        let mut lines = BufReader::new(client_read).lines();
        let line = lines.next_line().await.unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], 1);
        assert_eq!(
            value["error"]["code"],
            i32::from(agent_client_protocol::ErrorCode::RequestCancelled)
        );
        drop(writer);
    };

    let (boundary_result, ()) = tokio::join!(boundary, client);
    assert!(matches!(boundary_result, Ok(BoundaryExit::TransportClosed)));
}

#[tokio::test]
async fn every_agent_method_is_routed_through_the_boundary() {
    let (client_io, server_io) = tokio::io::duplex(16384);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let boundary = connect_agent_boundary(
        Arc::new(StubAgent),
        async_compat::Compat::new(server_write),
        async_compat::Compat::new(server_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    );

    let requests: Vec<(&str, serde_json::Value, bool)> = vec![
        ("initialize", serde_json::json!({"protocolVersion": 1}), true),
        ("authenticate", serde_json::json!({"methodId": "none"}), true),
        (
            "session/new",
            serde_json::json!({"cwd": "/tmp", "mcpServers": []}),
            true,
        ),
        (
            "session/load",
            serde_json::to_value(LoadSessionRequest::new("s1", "/tmp")).unwrap(),
            false,
        ),
        (
            "session/prompt",
            serde_json::json!({"sessionId": "s1", "prompt": []}),
            true,
        ),
        (
            "session/set_mode",
            serde_json::to_value(SetSessionModeRequest::new("s1", "mode")).unwrap(),
            false,
        ),
        (
            "session/set_config_option",
            serde_json::to_value(SetSessionConfigOptionRequest::new(
                "s1",
                "verbose",
                SessionConfigOptionValue::boolean(true),
            ))
            .unwrap(),
            false,
        ),
        (
            "session/fork",
            serde_json::to_value(ForkSessionRequest::new("s1", ".")).unwrap(),
            false,
        ),
        (
            "session/resume",
            serde_json::to_value(ResumeSessionRequest::new("s1", ".")).unwrap(),
            false,
        ),
        (
            "session/close",
            serde_json::to_value(CloseSessionRequest::new("s1")).unwrap(),
            false,
        ),
        (
            "session/delete",
            serde_json::to_value(DeleteSessionRequest::new("s1")).unwrap(),
            false,
        ),
        (
            "session/list",
            serde_json::to_value(ListSessionsRequest::new()).unwrap(),
            false,
        ),
        ("logout", serde_json::to_value(LogoutRequest::new()).unwrap(), false),
        ("_test/method", serde_json::json!({}), false),
    ];
    let expected: std::collections::HashMap<u64, bool> = requests
        .iter()
        .enumerate()
        .map(|(i, (_, _, ok))| ((i + 1) as u64, *ok))
        .collect();

    let client = async move {
        let mut writer = client_write;
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/cancel\",\"params\":{\"sessionId\":\"s1\"}}\n")
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"_test/note\",\"params\":{}}\n")
            .await
            .unwrap();
        for (i, (method, params, _)) in requests.iter().enumerate() {
            let frame = serde_json::json!({"jsonrpc": "2.0", "id": (i + 1) as u64, "method": method, "params": params});
            writer.write_all(format!("{frame}\n").as_bytes()).await.unwrap();
        }

        let mut lines = BufReader::new(client_read).lines();
        let mut seen = std::collections::HashMap::new();
        while seen.len() < expected.len() {
            let line = lines.next_line().await.unwrap().unwrap();
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            let Some(id) = value["id"].as_u64() else {
                continue;
            };
            seen.insert(
                id,
                value["result"].is_object() || value["result"].is_null() && value.get("error").is_none(),
            );
            if value.get("error").is_some() {
                assert_eq!(
                    value["error"]["code"],
                    i32::from(agent_client_protocol::ErrorCode::MethodNotFound),
                    "unexpected error for id {id}: {line}"
                );
            }
        }
        for (id, expect_ok) in &expected {
            let got_ok = seen[id];
            assert_eq!(got_ok, *expect_ok, "id {id} success mismatch");
        }
        drop(writer);
    };

    let (boundary_result, ()) = tokio::join!(boundary, client);
    assert!(matches!(boundary_result, Ok(BoundaryExit::TransportClosed)));
}

#[tokio::test]
async fn connection_client_forwards_every_client_method() {
    let (client_io, server_io) = tokio::io::duplex(16384);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    fn empty_raw_value() -> std::sync::Arc<serde_json::value::RawValue> {
        serde_json::value::RawValue::from_string("{}".to_string())
            .unwrap()
            .into()
    }

    let boundary = connect_agent_boundary(
        Arc::new(StubAgent),
        async_compat::Compat::new(server_write),
        async_compat::Compat::new(server_read),
        async move |cx| {
            let client = ConnectionClient::new(cx);

            let tool_call = ToolCallUpdate::new("call-1", ToolCallUpdateFields::new());
            let permission = client
                .request_permission(RequestPermissionRequest::new("s1", tool_call, vec![]))
                .await?;
            assert!(matches!(permission.outcome, RequestPermissionOutcome::Cancelled));

            client
                .session_notification(SessionNotification::new(
                    "s1",
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hi"))),
                ))
                .await?;

            let read = client.read_text_file(ReadTextFileRequest::new("s1", "/tmp/f")).await?;
            assert_eq!(read.content, "content");

            client
                .write_text_file(WriteTextFileRequest::new("s1", "/tmp/f", "content"))
                .await?;

            let terminal = client.create_terminal(CreateTerminalRequest::new("s1", "echo")).await?;
            assert_eq!(terminal.terminal_id.to_string(), "t1");

            let output = client.terminal_output(TerminalOutputRequest::new("s1", "t1")).await?;
            assert_eq!(output.output, "out");

            client.release_terminal(ReleaseTerminalRequest::new("s1", "t1")).await?;
            client
                .wait_for_terminal_exit(WaitForTerminalExitRequest::new("s1", "t1"))
                .await?;
            client.kill_terminal(KillTerminalRequest::new("s1", "t1")).await?;

            let ext = client.ext_method(ExtRequest::new("_probe", empty_raw_value())).await?;
            let ext_value: serde_json::Value = serde_json::from_str(ext.0.get())?;
            assert_eq!(ext_value["ok"], true);

            client
                .ext_notification(ExtNotification::new("_note", empty_raw_value()))
                .await?;

            let mode = ElicitationFormMode::new(ElicitationSessionScope::new("s1"), ElicitationSchema::new());
            client
                .elicitation_create(CreateElicitationRequest::new(mode, "please respond"))
                .await?;
            client
                .elicitation_complete(CompleteElicitationNotification::new("e1"))
                .await?;

            Ok(())
        },
    );

    let peer = async move {
        let mut writer = client_write;
        let mut lines = BufReader::new(client_read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            let (Some(id), Some(method)) = (value.get("id").cloned(), value["method"].as_str()) else {
                continue;
            };
            let result = match method {
                "session/request_permission" => {
                    serde_json::to_value(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)).unwrap()
                }
                "fs/read_text_file" => serde_json::to_value(ReadTextFileResponse::new("content")).unwrap(),
                "fs/write_text_file" => serde_json::to_value(WriteTextFileResponse::new()).unwrap(),
                "terminal/create" => serde_json::to_value(CreateTerminalResponse::new("t1")).unwrap(),
                "terminal/output" => {
                    serde_json::to_value(TerminalOutputResponse::new("out".to_string(), false)).unwrap()
                }
                "terminal/release" => serde_json::to_value(ReleaseTerminalResponse::new()).unwrap(),
                "terminal/wait_for_exit" => {
                    serde_json::to_value(WaitForTerminalExitResponse::new(TerminalExitStatus::new())).unwrap()
                }
                "terminal/kill" => serde_json::to_value(KillTerminalResponse::new()).unwrap(),
                "elicitation/create" => {
                    serde_json::to_value(CreateElicitationResponse::new(ElicitationAcceptAction::new())).unwrap()
                }
                _ => serde_json::json!({"ok": true}),
            };
            let frame = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
            writer.write_all(format!("{frame}\n").as_bytes()).await.unwrap();
        }
    };

    let (boundary_result, ()) = tokio::join!(boundary, peer);
    assert!(matches!(boundary_result, Ok(BoundaryExit::Main(()))));
}

struct ContraryAgent;

#[async_trait::async_trait]
impl AgentHandler for ContraryAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Err(Error::internal_error())
    }

    async fn ext_method(
        &self,
        _args: agent_client_protocol::schema::v1::ExtRequest,
    ) -> Result<agent_client_protocol::schema::v1::ExtResponse> {
        Ok(agent_client_protocol::schema::v1::ExtResponse::new(
            std::sync::Arc::from(serde_json::value::RawValue::from_string("{\"ok\":true}".to_string()).unwrap()),
        ))
    }
}

#[tokio::test]
async fn boundary_covers_ext_success_and_notification_fallthroughs() {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (server_read, server_write) = tokio::io::split(server_io);

    let boundary = connect_agent_boundary(
        Arc::new(ContraryAgent),
        async_compat::Compat::new(server_write),
        async_compat::Compat::new(server_read),
        async move |_cx| std::future::pending::<Result<()>>().await,
    );

    let client = async move {
        let mut writer = client_write;

        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/cancel\",\"params\":{\"sessionId\":\"s1\"}}\n")
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"mcp/message\",\"params\":{\"connectionId\":\"c1\",\"method\":\"tools/list\"}}\n")
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"_bad/note\",\"params\":{}}\n")
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"mcp/connect\",\"params\":{\"sessionId\":\"s1\",\"serverId\":\"srv-1\"}}\n")
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"_probe\",\"params\":{}}\n")
            .await
            .unwrap();

        let mut lines = BufReader::new(client_read).lines();
        let mut seen = std::collections::HashMap::new();
        while seen.len() < 2 {
            let line = lines.next_line().await.unwrap().unwrap();
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            let Some(id) = value["id"].as_u64() else {
                continue;
            };
            seen.insert(id, value);
        }
        assert_eq!(
            seen[&1]["error"]["code"],
            i32::from(agent_client_protocol::ErrorCode::MethodNotFound)
        );
        assert_eq!(seen[&2]["result"]["ok"], true);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drop(writer);
    };

    let (boundary_result, ()) = tokio::join!(boundary, client);
    assert!(matches!(boundary_result, Ok(BoundaryExit::TransportClosed)));
}
