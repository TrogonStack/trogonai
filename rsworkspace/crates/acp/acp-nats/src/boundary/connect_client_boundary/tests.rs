use super::*;
use crate::agent_handler::AgentHandler;
use crate::boundary::{ConnectionAgent, ConnectionClient, connect_agent_boundary};
use agent_client_protocol::ByteStreams;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, ContentBlock, ContentChunk, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
    PromptResponse, ReadTextFileRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, SessionUpdate, StopReason, TextContent, ToolCallUpdate, ToolCallUpdateFields,
};
use std::sync::{Mutex, OnceLock};

/// An agent that, during a prompt, reports output and asks permission through
/// the client it is connected to: the two callbacks a host must relay.
struct ChattyAgent {
    client: OnceLock<ConnectionClient>,
}

impl ChattyAgent {
    fn client(&self) -> Result<&ConnectionClient> {
        self.client.get().ok_or_else(Error::internal_error)
    }
}

#[async_trait::async_trait]
impl AgentHandler for ChattyAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new("sess-1"))
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        use crate::client_handler::ClientHandler as _;
        let client = self.client()?;
        client
            .session_notification(SessionNotification::new(
                args.session_id.clone(),
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new("hello")))),
            ))
            .await?;
        let answer = client
            .request_permission(RequestPermissionRequest::new(
                args.session_id,
                ToolCallUpdate::new("call-1", ToolCallUpdateFields::new()),
                vec![PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce)],
            ))
            .await?;
        let stop = match answer.outcome {
            RequestPermissionOutcome::Cancelled => StopReason::Cancelled,
            _ => StopReason::EndTurn,
        };
        Ok(PromptResponse::new(stop))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingClient {
    updates: Mutex<Vec<SessionNotification>>,
    permission_asks: Mutex<u32>,
}

#[async_trait::async_trait]
impl ClientHandler for RecordingClient {
    async fn request_permission(&self, _args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        *self.permission_asks.lock().map_err(|_| Error::internal_error())? += 1;
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        self.updates.lock().map_err(|_| Error::internal_error())?.push(args);
        Ok(())
    }
}

/// Wires a [`ChattyAgent`] behind the agent boundary on one end of an in-memory
/// pipe and returns the byte-stream transport for the client end.
fn spawn_chatty_agent()
-> ByteStreams<impl futures::AsyncWrite + Send + 'static, impl futures::AsyncRead + Send + 'static> {
    let (client_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (agent_read, agent_write) = tokio::io::split(agent_io);

    let agent = Arc::new(ChattyAgent {
        client: OnceLock::new(),
    });
    tokio::spawn(async move {
        let agent_for_main = agent.clone();
        let _ = connect_agent_boundary(
            agent,
            async_compat::Compat::new(agent_write),
            async_compat::Compat::new(agent_read),
            async move |cx| {
                let _ = agent_for_main.client.set(ConnectionClient::new(cx));
                std::future::pending::<Result<()>>().await
            },
        )
        .await;
    });

    ByteStreams::new(
        async_compat::Compat::new(client_write),
        async_compat::Compat::new(client_read),
    )
}

#[tokio::test]
async fn a_spawned_agents_updates_and_permission_asks_reach_the_client_handler() {
    let client = Arc::new(RecordingClient::default());

    let exit = connect_client_boundary_with(client.clone(), spawn_chatty_agent(), async move |cx| {
        let agent = ConnectionAgent::new(cx);
        agent.initialize(InitializeRequest::new(ProtocolVersion::V1)).await?;
        let session = agent.new_session(NewSessionRequest::new("/tmp")).await?;
        agent
            .prompt(PromptRequest::new(
                session.session_id,
                vec![ContentBlock::Text(TextContent::new("hi"))],
            ))
            .await
    })
    .await
    .expect("boundary runs");

    let BoundaryExit::Main(response) = exit else {
        panic!("connection closed before the prompt answered");
    };
    assert_eq!(response.stop_reason, StopReason::Cancelled);
    assert_eq!(client.updates.lock().expect("lock").len(), 1);
    assert_eq!(*client.permission_asks.lock().expect("lock"), 1);
}

#[tokio::test]
async fn an_agent_request_the_client_does_not_serve_is_answered_method_not_found() {
    let client = Arc::new(RecordingClient::default());
    let (client_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let (agent_read, agent_write) = tokio::io::split(agent_io);

    tokio::spawn(connect_client_boundary_with(
        client,
        ByteStreams::new(
            async_compat::Compat::new(client_write),
            async_compat::Compat::new(client_read),
        ),
        async move |_cx| std::future::pending::<Result<()>>().await,
    ));

    // A raw agent asks the client to read a file, which `RecordingClient`
    // leaves at the trait's `method_not_found` default.
    let request = ReadTextFileRequest::new("sess-1", "/etc/hosts");
    let line = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "fs/read_text_file",
        "params": request,
    });
    let mut agent_write = agent_write;
    tokio::io::AsyncWriteExt::write_all(&mut agent_write, format!("{line}\n").as_bytes())
        .await
        .expect("write");

    let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(agent_read));
    let reply = loop {
        let raw = lines.next_line().await.expect("read").expect("a reply line");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
        if value.get("id") == Some(&serde_json::json!(7)) {
            break value;
        }
    };
    assert_eq!(reply["error"]["code"], serde_json::json!(-32601));
}

#[tokio::test]
async fn the_boundary_ends_when_the_agent_closes_its_side() {
    let client = Arc::new(RecordingClient::default());
    let exit = connect_client_boundary_with(
        client,
        ByteStreams::new(futures::io::sink(), futures::io::empty()),
        async move |_cx| std::future::pending::<Result<()>>().await,
    )
    .await
    .expect("boundary runs");

    assert!(matches!(exit, BoundaryExit::TransportClosed));
}

#[test]
fn unknown_agent_methods_are_kept_for_rejection_rather_than_failing_the_parse() {
    let parsed = AgentBoundaryRequest::parse_message("vendor/unknown", &serde_json::json!({})).expect("parses");
    assert!(matches!(parsed, AgentBoundaryRequest::Unsupported(_)));
    assert_eq!(parsed.method(), "vendor/unknown");
}
