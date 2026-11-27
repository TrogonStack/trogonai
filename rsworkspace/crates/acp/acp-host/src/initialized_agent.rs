use crate::client_claim::ClientClaim;
use acp_nats::AgentHandler;
use agent_client_protocol::Result;
use agent_client_protocol::v1::schema::{
    ArchiveSessionRequest, ArchiveSessionResponse, AuthStatusRequest, AuthStatusResponse, AuthenticateRequest,
    AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest,
    DeleteSessionResponse, DisableProviderRequest, DisableProviderResponse, ExtNotification, ExtRequest, ExtResponse,
    ForkSessionRequest, ForkSessionResponse, InitializeRequest, InitializeResponse, ListProvidersRequest,
    ListProvidersResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    LogoutRequest, LogoutResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionStatusRequest, SessionStatusResponse, SetProviderRequest,
    SetProviderResponse, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, SetSessionTitleRequest, SetSessionTitleResponse, UnarchiveSessionRequest,
    UnarchiveSessionResponse,
};
use std::sync::Arc;
use tracing::warn;

/// An agent the host has already initialized, served to its one client on NATS.
///
/// ACP has a client send `initialize` once per connection, but the host's
/// connection to the child outlives any one client connection: the client may
/// restart and initialize again while the child keeps running. Forwarding
/// every `initialize` would ask the agent to initialize repeatedly on one
/// connection, which the protocol does not promise to allow. The host
/// initializes the child once when it starts and answers the client's
/// `initialize` with that reply, after checking the [`ClientClaim`] so that a
/// second, different client is refused. Every other method passes through
/// untouched.
pub struct InitializedAgent<A> {
    inner: A,
    initialized: InitializeResponse,
    claim: Arc<ClientClaim>,
}

impl<A> InitializedAgent<A> {
    pub fn new(inner: A, initialized: InitializeResponse, claim: Arc<ClientClaim>) -> Self {
        Self {
            inner,
            initialized,
            claim,
        }
    }
}

#[async_trait::async_trait]
impl<A: AgentHandler + Send + Sync> AgentHandler for InitializedAgent<A> {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        if let Err(refusal) = self.claim.admit(&args) {
            warn!(error = %refusal, "refused a second client's initialize");
            return Err(refusal);
        }
        Ok(self.initialized.clone())
    }

    async fn auth_status(&self, args: AuthStatusRequest) -> Result<AuthStatusResponse> {
        self.inner.auth_status(args).await
    }

    async fn archive_session(&self, args: ArchiveSessionRequest) -> Result<ArchiveSessionResponse> {
        self.inner.archive_session(args).await
    }

    async fn unarchive_session(&self, args: UnarchiveSessionRequest) -> Result<UnarchiveSessionResponse> {
        self.inner.unarchive_session(args).await
    }

    async fn session_status(&self, args: SessionStatusRequest) -> Result<SessionStatusResponse> {
        self.inner.session_status(args).await
    }

    async fn set_session_title(&self, args: SetSessionTitleRequest) -> Result<SetSessionTitleResponse> {
        self.inner.set_session_title(args).await
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        self.inner.authenticate(args).await
    }

    async fn logout(&self, args: LogoutRequest) -> Result<LogoutResponse> {
        self.inner.logout(args).await
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        self.inner.new_session(args).await
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        self.inner.load_session(args).await
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        self.inner.prompt(args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        self.inner.cancel(args).await
    }

    async fn set_session_mode(&self, args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        self.inner.set_session_mode(args).await
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        self.inner.set_session_config_option(args).await
    }

    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        self.inner.fork_session(args).await
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        self.inner.resume_session(args).await
    }

    async fn close_session(&self, args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        self.inner.close_session(args).await
    }

    async fn delete_session(&self, args: DeleteSessionRequest) -> Result<DeleteSessionResponse> {
        self.inner.delete_session(args).await
    }

    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        self.inner.list_sessions(args).await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        self.inner.ext_method(args).await
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        self.inner.ext_notification(args).await
    }

    async fn list_providers(&self, args: ListProvidersRequest) -> Result<ListProvidersResponse> {
        self.inner.list_providers(args).await
    }

    async fn set_provider(&self, args: SetProviderRequest) -> Result<SetProviderResponse> {
        self.inner.set_provider(args).await
    }

    async fn disable_provider(&self, args: DisableProviderRequest) -> Result<DisableProviderResponse> {
        self.inner.disable_provider(args).await
    }
}

#[cfg(test)]
mod tests;
