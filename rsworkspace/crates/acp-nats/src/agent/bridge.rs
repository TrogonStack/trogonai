use crate::config::Config;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest,
    ForkSessionResponse, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, Result, ResumeSessionRequest,
    ResumeSessionResponse, SessionNotification, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse,
};
use opentelemetry::metrics::Meter;
use tokio::sync::mpsc;
use trogon_std::time::GetElapsed;

use super::{
    authenticate, cancel, close_session, ext_method, ext_notification, fork_session, initialize,
    list_sessions, load_session, new_session, prompt, resume_session, set_session_config_option,
    set_session_mode, set_session_model,
};

pub struct Bridge<N, C: GetElapsed> {
    pub(crate) nats: N,
    pub(crate) clock: C,
    pub(crate) config: Config,
    pub(crate) metrics: Metrics,
    pub(crate) notification_sender: mpsc::Sender<SessionNotification>,
}

impl<N, C: GetElapsed> Bridge<N, C> {
    #[cfg_attr(coverage, coverage(off))]
    pub fn new(
        nats: N,
        clock: C,
        meter: &Meter,
        config: Config,
        notification_sender: mpsc::Sender<SessionNotification>,
    ) -> Self {
        Self {
            nats,
            clock,
            config,
            metrics: Metrics::new(meter),
            notification_sender,
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + SubscribeClient + FlushClient, C: GetElapsed> Agent
    for Bridge<N, C>
{
    #[cfg_attr(coverage, coverage(off))]
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        initialize::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        authenticate::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        new_session::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        load_session::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        set_session_mode::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        prompt::handle(self, args, &trogon_std::StdJsonSerialize).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        cancel::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        list_sessions::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        set_session_config_option::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse> {
        set_session_model::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        fork_session::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        resume_session::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn close_session(&self, args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        close_session::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        ext_method::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        ext_notification::handle(self, args).await
    }
}
