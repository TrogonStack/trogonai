use acp_nats::{AcpPrefix, AcpSessionId, ClientHandler, FlushClient, NatsClientProxy, PublishClient, RequestClient};
use agent_client_protocol::v1::schema::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionRequest,
    RequestPermissionResponse, SessionId, SessionNotification, TerminalOutputRequest, TerminalOutputResponse,
    WaitForTerminalExitRequest, WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::{Error, Result};
use std::time::Duration;

/// The client a spawned agent talks to, as seen from the host.
///
/// One child serves many sessions, but `acp-nats` addresses client callbacks
/// per session (`{prefix}.session.{id}.client.*`). Every callback an agent
/// makes names its session, so this routes each one to a
/// [`NatsClientProxy`] for that session: `session/update` lands on the
/// session's `client.session.update` subject (captured by the `CLIENT_OPS`
/// stream), and requests such as `session/request_permission` reach whichever
/// client opened the session.
///
/// Elicitation and extension callbacks carry no session, so they keep the
/// trait's `method_not_found` defaults.
pub struct SessionRoutedClient<N> {
    nats: N,
    prefix: AcpPrefix,
    operation_timeout: Duration,
}

impl<N: Clone> SessionRoutedClient<N> {
    pub fn new(nats: N, prefix: AcpPrefix, operation_timeout: Duration) -> Self {
        Self {
            nats,
            prefix,
            operation_timeout,
        }
    }

    fn for_session(&self, session_id: &SessionId) -> Result<NatsClientProxy<N>> {
        // An id that is not a valid NATS token cannot be addressed at all; the
        // agent chose it, so it hears about it as bad parameters.
        let session = AcpSessionId::try_from(session_id).map_err(|_| Error::invalid_params())?;
        Ok(NatsClientProxy::new(
            self.nats.clone(),
            session,
            self.prefix.clone(),
            self.operation_timeout,
        ))
    }
}

#[async_trait::async_trait]
impl<N> ClientHandler for SessionRoutedClient<N>
where
    N: RequestClient + PublishClient + FlushClient + Clone + Send + Sync + 'static,
{
    async fn request_permission(&self, args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        self.for_session(&args.session_id)?.request_permission(args).await
    }

    async fn session_notification(&self, args: SessionNotification) -> Result<()> {
        self.for_session(&args.session_id)?.session_notification(args).await
    }

    async fn read_text_file(&self, args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        self.for_session(&args.session_id)?.read_text_file(args).await
    }

    async fn write_text_file(&self, args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        self.for_session(&args.session_id)?.write_text_file(args).await
    }

    async fn create_terminal(&self, args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        self.for_session(&args.session_id)?.create_terminal(args).await
    }

    async fn terminal_output(&self, args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        self.for_session(&args.session_id)?.terminal_output(args).await
    }

    async fn release_terminal(&self, args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        self.for_session(&args.session_id)?.release_terminal(args).await
    }

    async fn wait_for_terminal_exit(&self, args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        self.for_session(&args.session_id)?.wait_for_terminal_exit(args).await
    }

    async fn kill_terminal(&self, args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        self.for_session(&args.session_id)?.kill_terminal(args).await
    }
}

#[cfg(test)]
mod tests;
