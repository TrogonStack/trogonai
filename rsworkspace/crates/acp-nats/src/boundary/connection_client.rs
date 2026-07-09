use crate::client_handler::ClientHandler;
use agent_client_protocol::schema::v1::{
    CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse, CreateTerminalRequest,
    CreateTerminalResponse, ExtNotification, ExtRequest, ExtResponse, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::{Client, ConnectionTo, Error, Result, UntypedMessage};

/// Adapts a connection handle to the bridge-owned [`ClientHandler`] trait.
///
/// See ADR 0020: this is the SDK-connection-aware boundary shared by
/// `acp-nats-server` and `acp-nats-stdio`. Every method is a zero-cost
/// passthrough to the connection's outbound request/notification calls.
#[derive(Clone)]
pub struct ConnectionClient {
    cx: ConnectionTo<Client>,
}

impl ConnectionClient {
    pub fn new(cx: ConnectionTo<Client>) -> Self {
        Self { cx }
    }
}

#[async_trait::async_trait]
impl ClientHandler for ConnectionClient {
    async fn request_permission(&self, args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        self.cx.send_notification(args)
    }

    async fn read_text_file(&self, args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn write_text_file(&self, args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn create_terminal(&self, args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn terminal_output(&self, args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn release_terminal(&self, args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn wait_for_terminal_exit(&self, args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn kill_terminal(&self, args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        let wire_method = format!("_{}", args.method);
        let value = self
            .cx
            .send_request(UntypedMessage::new(&wire_method, args.params)?)
            .block_task()
            .await?;
        let raw_value = serde_json::value::to_raw_value(&value).map_err(Error::into_internal_error)?;
        Ok(ExtResponse::new(raw_value.into()))
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        let wire_method = format!("_{}", args.method);
        self.cx
            .send_notification(UntypedMessage::new(&wire_method, args.params)?)
    }

    async fn elicitation_create(&self, args: CreateElicitationRequest) -> Result<CreateElicitationResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn elicitation_complete(&self, args: CompleteElicitationNotification) -> Result<()> {
        self.cx.send_notification(args)
    }
}
