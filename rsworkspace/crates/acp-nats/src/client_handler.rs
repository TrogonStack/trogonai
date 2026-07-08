use agent_client_protocol::schema::v1::{
    CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse, CreateTerminalRequest,
    CreateTerminalResponse, ExtNotification, ExtRequest, ExtResponse, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::{Error, Result};

/// Bridge-owned replacement for the SDK's removed `Client` trait.
///
/// See ADR 0020 for why this trait exists instead of implementing an SDK trait directly.
#[async_trait::async_trait]
pub trait ClientHandler {
    async fn request_permission(&self, args: RequestPermissionRequest) -> Result<RequestPermissionResponse>;

    async fn session_notification(&self, args: SessionNotification) -> Result<()>;

    async fn read_text_file(&self, _args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        Err(Error::method_not_found())
    }

    async fn write_text_file(&self, _args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        Err(Error::method_not_found())
    }

    async fn create_terminal(&self, _args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        Err(Error::method_not_found())
    }

    async fn terminal_output(&self, _args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        Err(Error::method_not_found())
    }

    async fn release_terminal(&self, _args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        Err(Error::method_not_found())
    }

    async fn wait_for_terminal_exit(&self, _args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        Err(Error::method_not_found())
    }

    async fn kill_terminal(&self, _args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        Err(Error::method_not_found())
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        Err(Error::method_not_found())
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Err(Error::method_not_found())
    }

    async fn elicitation_create(&self, _args: CreateElicitationRequest) -> Result<CreateElicitationResponse> {
        Err(Error::method_not_found())
    }

    async fn elicitation_complete(&self, _args: CompleteElicitationNotification) -> Result<()> {
        Err(Error::method_not_found())
    }
}
