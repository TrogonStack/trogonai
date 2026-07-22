use agent_client_protocol::schema::v1::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    DeleteSessionRequest, DeleteSessionResponse, DisableProviderRequest, DisableProviderResponse, ExtNotification,
    ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse, InitializeRequest, InitializeResponse,
    ListProvidersRequest, ListProvidersResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, LogoutRequest, LogoutResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, ResumeSessionRequest, ResumeSessionResponse, SetProviderRequest, SetProviderResponse,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
};
use agent_client_protocol::{Error, Result};

/// Bridge-owned replacement for the SDK's removed `Agent` trait.
///
/// See ADR#0020 for why this trait exists instead of implementing an SDK trait directly.
#[async_trait::async_trait]
pub trait AgentHandler {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse>;

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse>;

    async fn logout(&self, _args: LogoutRequest) -> Result<LogoutResponse> {
        Err(Error::method_not_found())
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse>;

    async fn load_session(&self, _args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse>;

    async fn cancel(&self, args: CancelNotification) -> Result<()>;

    async fn set_session_mode(&self, _args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        Err(Error::method_not_found())
    }

    async fn set_session_config_option(
        &self,
        _args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        Err(Error::method_not_found())
    }

    async fn fork_session(&self, _args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn resume_session(&self, _args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn close_session(&self, _args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn delete_session(&self, _args: DeleteSessionRequest) -> Result<DeleteSessionResponse> {
        Err(Error::method_not_found())
    }

    async fn list_sessions(&self, _args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        Err(Error::method_not_found())
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        Err(Error::method_not_found())
    }

    async fn ext_notification(&self, _args: ExtNotification) -> Result<()> {
        Err(Error::method_not_found())
    }

    async fn list_providers(&self, _args: ListProvidersRequest) -> Result<ListProvidersResponse> {
        Err(Error::method_not_found())
    }

    async fn set_provider(&self, _args: SetProviderRequest) -> Result<SetProviderResponse> {
        Err(Error::method_not_found())
    }

    async fn disable_provider(&self, _args: DisableProviderRequest) -> Result<DisableProviderResponse> {
        Err(Error::method_not_found())
    }
}

#[cfg(test)]
mod tests;
