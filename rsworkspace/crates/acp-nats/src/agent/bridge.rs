use std::cell::RefCell;

use crate::config::Config;
use crate::nats::{
    self, ExtSessionReady, FlushClient, FlushPolicy, PublishClient, PublishOptions, RequestClient, RetryPolicy,
    SubscribeClient, session,
};
use crate::pending_prompt_waiters::PendingSessionPromptResponseWaiters;
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest, ForkSessionResponse, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    LogoutRequest, LogoutResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, Result,
    ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionNotification, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse, SetSessionModelRequest,
    SetSessionModelResponse,
};
use opentelemetry::metrics::Meter;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_std::time::GetElapsed;

use super::{
    authenticate, cancel, close_session, ext_method, ext_notification, fork_session, initialize, js_request,
    list_sessions, load_session, logout, new_session, prompt, resume_session, set_session_config_option,
    set_session_mode, set_session_model,
};

use crate::constants::SESSION_READY_DELAY;

pub struct Bridge<N, C: GetElapsed, J> {
    pub(crate) nats: N,
    pub(crate) js: J,
    pub(crate) clock: C,
    pub(crate) config: Config,
    pub(crate) metrics: Metrics,
    pub(crate) notification_sender: mpsc::Sender<SessionNotification>,
    pub(crate) pending_session_prompt_responses: PendingSessionPromptResponseWaiters<C::Instant>,
    pub(crate) background_tasks: RefCell<Vec<JoinHandle<()>>>,
}

impl<N, C: GetElapsed, J> Bridge<N, C, J> {
    pub fn new(
        nats: N,
        js: J,
        clock: C,
        meter: &Meter,
        config: Config,
        notification_sender: mpsc::Sender<SessionNotification>,
    ) -> Self {
        Self {
            nats,
            js,
            clock,
            config,
            metrics: Metrics::new(meter),
            notification_sender,
            pending_session_prompt_responses: PendingSessionPromptResponseWaiters::new(),
            background_tasks: RefCell::new(Vec::new()),
        }
    }

    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }

    pub(crate) fn js(&self) -> &J {
        &self.js
    }

    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn spawn_background(&self, task: JoinHandle<()>) {
        self.background_tasks.borrow_mut().push(task);
    }

    #[cfg_attr(coverage, coverage(off))]
    pub async fn drain_background_tasks(&self) {
        let tasks: Vec<_> = self.background_tasks.borrow_mut().drain(..).collect();
        for task in tasks {
            let _ = task.await;
        }
    }
}

impl<N: PublishClient + FlushClient + Clone + Send + 'static, C: GetElapsed, J> Bridge<N, C, J> {
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn schedule_session_ready(&self, session_id: SessionId) {
        let nats = self.nats.clone();
        let prefix = self.config.acp_prefix_ref().clone();
        let metrics = self.metrics.clone();
        let handle = tokio::spawn(async move {
            publish_session_ready(&nats, &prefix, &session_id, &metrics).await;
        });
        self.spawn_background(handle);
    }
}

#[cfg_attr(coverage, coverage(off))]
async fn publish_session_ready<N: PublishClient + FlushClient>(
    nats: &N,
    prefix: &crate::acp_prefix::AcpPrefix,
    session_id: &SessionId,
    metrics: &Metrics,
) {
    tokio::time::sleep(SESSION_READY_DELAY).await;

    let acp_session_id = match crate::session_id::AcpSessionId::new(session_id.to_string()) {
        Ok(id) => id,
        Err(e) => {
            warn!(session_id = %session_id, error = %e, "Invalid session ID from backend, skipping session.ready");
            metrics.record_error("session_ready", "invalid_session_id");
            return;
        }
    };
    let subject = session::agent::ExtReadySubject::new(prefix, &acp_session_id);
    info!(session_id = %session_id, subject = %subject, "Publishing session.ready");

    let message = ExtSessionReady::new(session_id.clone());

    let options = PublishOptions::builder()
        .publish_retry_policy(RetryPolicy::standard())
        .flush_policy(FlushPolicy::standard())
        .build();

    if let Err(e) = nats::publish(nats, &subject, &message, options).await {
        warn!(
            error = %e,
            session_id = %session_id,
            "Failed to publish session.ready"
        );
        metrics.record_error("session_ready", "session_ready_publish_failed");
    } else {
        info!(session_id = %session_id, "Published session.ready");
    }
}

impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed, J: JetStreamPublisher + JetStreamGetStream>
    Bridge<N, C, J>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    pub(crate) async fn session_request<Req, Res>(
        &self,
        subject: &impl crate::nats::markers::SessionCommand,
        args: &Req,
        session_id: &crate::session_id::AcpSessionId,
    ) -> Result<Res>
    where
        Req: serde::Serialize,
        Res: serde::de::DeserializeOwned,
    {
        let subject_str = subject.to_string();
        let req_id = crate::req_id::ReqId::new();
        js_request::js_request::<J, _, Res, _>(
            self.js(),
            &subject_str,
            args,
            &trogon_std::StdJsonSerialize,
            self.config.acp_prefix_ref(),
            session_id,
            &req_id,
            self.config.operation_timeout,
        )
        .await
    }
}

#[async_trait::async_trait(?Send)]
impl<
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
> Agent for Bridge<N, C, J>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    #[cfg_attr(coverage, coverage(off))]
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        initialize::handle(self, args).await
    }

    #[cfg_attr(coverage, coverage(off))]
    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        authenticate::handle(self, args).await
    }

    async fn logout(&self, args: LogoutRequest) -> Result<LogoutResponse> {
        logout::handle(self, args).await
    }


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
