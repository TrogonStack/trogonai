use crate::agent_handler::AgentHandler;
use crate::ext_method_name::ext_wire_method;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AGENT_METHOD_NAMES, ArchiveSessionRequest, ArchiveSessionResponse, AuthStatusRequest, AuthStatusResponse,
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    DeleteSessionRequest, DeleteSessionResponse, DisableProviderRequest, DisableProviderResponse, ExtNotification,
    ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse, InitializeRequest, InitializeResponse,
    ListProvidersRequest, ListProvidersResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, LogoutRequest, LogoutResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, ResumeSessionRequest, ResumeSessionResponse, SessionStatusRequest, SessionStatusResponse,
    SetProviderRequest, SetProviderResponse, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    SetSessionModeRequest, SetSessionModeResponse, SetSessionTitleRequest, SetSessionTitleResponse,
    UnarchiveSessionRequest, UnarchiveSessionResponse,
};
use agent_client_protocol::{Agent, Error, Result, UntypedMessage};

/// Adapts a connection to an agent peer to the bridge-owned [`AgentHandler`] trait.
///
/// The mirror of [`ConnectionClient`](super::ConnectionClient): where that type
/// lets the bridge call *into* a client it is connected to, this one lets a host
/// call *into* an agent it spawned. Every method is a zero-cost passthrough to
/// the connection's outbound request/notification calls (ADR#0020).
#[derive(Clone)]
pub struct ConnectionAgent {
    cx: ConnectionTo<Agent>,
}

impl ConnectionAgent {
    pub fn new(cx: ConnectionTo<Agent>) -> Self {
        Self { cx }
    }
}

#[async_trait::async_trait]
impl AgentHandler for ConnectionAgent {
    async fn auth_status(&self, args: AuthStatusRequest) -> Result<AuthStatusResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn archive_session(&self, args: ArchiveSessionRequest) -> Result<ArchiveSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn unarchive_session(&self, args: UnarchiveSessionRequest) -> Result<UnarchiveSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn session_status(&self, args: SessionStatusRequest) -> Result<SessionStatusResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn set_session_title(&self, args: SetSessionTitleRequest) -> Result<SetSessionTitleResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn initialize_raw(&self, params: serde_json::Value) -> Result<InitializeResponse> {
        let value = self
            .cx
            .send_request(UntypedMessage {
                method: AGENT_METHOD_NAMES.initialize.to_string(),
                params,
            })
            .block_task()
            .await?;
        serde_json::from_value(value).map_err(Error::into_internal_error)
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn logout(&self, args: LogoutRequest) -> Result<LogoutResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        self.cx.send_notification(args)
    }

    async fn set_session_mode(&self, args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn close_session(&self, args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn delete_session(&self, args: DeleteSessionRequest) -> Result<DeleteSessionResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        let wire_method = ext_wire_method(&args.method);
        let value = self
            .cx
            .send_request(UntypedMessage::new(&wire_method, args.params)?)
            .block_task()
            .await?;
        let raw_value = serde_json::value::to_raw_value(&value).map_err(Error::into_internal_error)?;
        Ok(ExtResponse::new(raw_value.into()))
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        let wire_method = ext_wire_method(&args.method);
        self.cx
            .send_notification(UntypedMessage::new(&wire_method, args.params)?)
    }

    async fn list_providers(&self, args: ListProvidersRequest) -> Result<ListProvidersResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn set_provider(&self, args: SetProviderRequest) -> Result<SetProviderResponse> {
        self.cx.send_request(args).block_task().await
    }

    async fn disable_provider(&self, args: DisableProviderRequest) -> Result<DisableProviderResponse> {
        self.cx.send_request(args).block_task().await
    }
}

#[cfg(test)]
mod tests;
