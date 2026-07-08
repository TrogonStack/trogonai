use super::*;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, StopReason,
};

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
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
