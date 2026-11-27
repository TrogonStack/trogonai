use crate::client_claim::ClientClaim;
use crate::client_handshake::{ClientHandshake, ClientInitialize};
use crate::constants::FALLBACK_PROTOCOL_VERSION;
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
use tokio::sync::{Mutex, Notify};
use tracing::{info, warn};

/// The host's child agent, served to its one client on NATS, with the
/// `initialize` handshake owned by the host.
///
/// The child must see the client's own `initialize`, exactly as sent: its
/// protocol version, capabilities, `clientInfo` and `_meta` are how the client
/// tells the agent what it offers (fx reads the host tools it may call from
/// `clientCapabilities.libtrogonai`, a member the typed request would drop;
/// see [`ClientInitialize`]). But ACP
/// has a client send `initialize` once per connection, and the host's
/// connection to the child outlives any one client connection: the client may
/// restart and initialize again while the child keeps running. So:
///
/// - The first `initialize` this child sees is the client's, forwarded as sent,
///   and the client gets the child's real answer. It is remembered in the
///   [`ClientHandshake`], and every later child the host launches (after a
///   crash, say) is initialized with it before it serves anything.
/// - The same `initialize` again is answered from this child's reply, without
///   bothering the child.
/// - A different one (the client restarted with new capabilities, a new
///   version, or new `_meta`) is not sent to this child a second time: the
///   host relaunches the child, initializes the new one with it, and answers
///   with that child's reply.
/// - Any other request that reaches a child no client has initialized (the
///   host restarted and its client carried on without initializing again)
///   first initializes it with defaults at [`FALLBACK_PROTOCOL_VERSION`]; the
///   client's next `initialize` then relaunches it with the real parameters.
///
/// A second, different client is refused by the [`ClientClaim`] before
/// anything reaches the child: here for `initialize`, which names its client
/// in `clientInfo`, and on the NATS side for everything else, which names it
/// in a header (see [`Self::claim`]). Only an `initialize` the child answered
/// claims the host.
pub struct InitializedAgent<A> {
    inner: A,
    claim: Arc<ClientClaim>,
    handshake: Arc<ClientHandshake>,
    /// How this run's child was initialized, once it has been.
    child: Mutex<Option<Handshake>>,
    relaunch: Arc<Notify>,
}

/// One `initialize` the child answered.
#[derive(Debug, Clone)]
pub struct Handshake {
    pub request: ClientInitialize,
    pub response: InitializeResponse,
}

impl<A> InitializedAgent<A> {
    /// `initialized` is the handshake the run already did with the child, if
    /// any. `relaunch` is notified when a client asks for a new child.
    pub fn new(
        inner: A,
        initialized: Option<Handshake>,
        claim: Arc<ClientClaim>,
        handshake: Arc<ClientHandshake>,
        relaunch: Arc<Notify>,
    ) -> Self {
        Self {
            inner,
            claim,
            handshake,
            child: Mutex::new(initialized),
            relaunch,
        }
    }

    /// The claim this agent is served under, to gate the requests that do not
    /// pass through `initialize`.
    pub fn claim(&self) -> Arc<ClientClaim> {
        self.claim.clone()
    }
}

impl<A: AgentHandler + Send + Sync> InitializedAgent<A> {
    async fn initialize_client(&self, args: ClientInitialize) -> Result<InitializeResponse> {
        if let Err(refusal) = self.claim.admit(args.typed()) {
            warn!(error = %refusal, "refused a second client's initialize");
            return Err(refusal);
        }
        let mut child = self.child.lock().await;
        let response = match child.as_ref() {
            Some(done) if done.request == args => done.response.clone(),
            Some(_) => {
                info!("the client initialized with different parameters; relaunching the agent with them");
                let pending = self.handshake.listen();
                self.handshake.remember(args.clone());
                self.relaunch.notify_one();
                drop(child);
                pending.answer(&args).await?
            }
            None => {
                let response = self.inner.initialize_raw(args.params().clone()).await?;
                info!(agent = ?response.agent_info, "agent initialized by the client");
                self.handshake.remember(args.clone());
                *child = Some(Handshake {
                    request: args.clone(),
                    response: response.clone(),
                });
                response
            }
        };
        if let Err(refusal) = self.claim.claim(args.typed()) {
            warn!(error = %refusal, "another client claimed the host while this initialize was answered");
            return Err(refusal);
        }
        Ok(response)
    }

    /// Makes sure the child is initialized before it gets any other request.
    async fn ready(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        if child.is_none() {
            warn!(
                "a request reached the agent before any client initialized it; initializing it with defaults \
                 until the client initializes"
            );
            let request = ClientInitialize::from(InitializeRequest::new(FALLBACK_PROTOCOL_VERSION));
            let response = self.inner.initialize_raw(request.params().clone()).await?;
            *child = Some(Handshake { request, response });
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl<A: AgentHandler + Send + Sync> AgentHandler for InitializedAgent<A> {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.initialize_client(args.into()).await
    }

    async fn initialize_raw(&self, params: serde_json::Value) -> Result<InitializeResponse> {
        self.initialize_client(ClientInitialize::from_params(params)?).await
    }

    async fn auth_status(&self, args: AuthStatusRequest) -> Result<AuthStatusResponse> {
        self.ready().await?;
        self.inner.auth_status(args).await
    }

    async fn archive_session(&self, args: ArchiveSessionRequest) -> Result<ArchiveSessionResponse> {
        self.ready().await?;
        self.inner.archive_session(args).await
    }

    async fn unarchive_session(&self, args: UnarchiveSessionRequest) -> Result<UnarchiveSessionResponse> {
        self.ready().await?;
        self.inner.unarchive_session(args).await
    }

    async fn session_status(&self, args: SessionStatusRequest) -> Result<SessionStatusResponse> {
        self.ready().await?;
        self.inner.session_status(args).await
    }

    async fn set_session_title(&self, args: SetSessionTitleRequest) -> Result<SetSessionTitleResponse> {
        self.ready().await?;
        self.inner.set_session_title(args).await
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        self.ready().await?;
        self.inner.authenticate(args).await
    }

    async fn logout(&self, args: LogoutRequest) -> Result<LogoutResponse> {
        self.ready().await?;
        self.inner.logout(args).await
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        self.ready().await?;
        self.inner.new_session(args).await
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        self.ready().await?;
        self.inner.load_session(args).await
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        self.ready().await?;
        self.inner.prompt(args).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        self.ready().await?;
        self.inner.cancel(args).await
    }

    async fn set_session_mode(&self, args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        self.ready().await?;
        self.inner.set_session_mode(args).await
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        self.ready().await?;
        self.inner.set_session_config_option(args).await
    }

    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        self.ready().await?;
        self.inner.fork_session(args).await
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        self.ready().await?;
        self.inner.resume_session(args).await
    }

    async fn close_session(&self, args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        self.ready().await?;
        self.inner.close_session(args).await
    }

    async fn delete_session(&self, args: DeleteSessionRequest) -> Result<DeleteSessionResponse> {
        self.ready().await?;
        self.inner.delete_session(args).await
    }

    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        self.ready().await?;
        self.inner.list_sessions(args).await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        self.ready().await?;
        self.inner.ext_method(args).await
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        self.ready().await?;
        self.inner.ext_notification(args).await
    }

    async fn list_providers(&self, args: ListProvidersRequest) -> Result<ListProvidersResponse> {
        self.ready().await?;
        self.inner.list_providers(args).await
    }

    async fn set_provider(&self, args: SetProviderRequest) -> Result<SetProviderResponse> {
        self.ready().await?;
        self.inner.set_provider(args).await
    }

    async fn disable_provider(&self, args: DisableProviderRequest) -> Result<DisableProviderResponse> {
        self.ready().await?;
        self.inner.disable_provider(args).await
    }
}

#[cfg(test)]
mod tests;
