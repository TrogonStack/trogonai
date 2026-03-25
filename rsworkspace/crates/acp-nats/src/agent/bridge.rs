use std::cell::RefCell;
use std::time::Duration;

use crate::config::Config;
use crate::nats::{
    self, ExtSessionReady, FlushClient, FlushPolicy, PublishClient, PublishOptions, RequestClient,
    RetryPolicy, SubscribeClient, agent,
};
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, CloseSessionRequest,
    CloseSessionResponse, ExtNotification, ExtRequest, ExtResponse, ForkSessionRequest,
    ForkSessionResponse, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, Result, ResumeSessionRequest,
    ResumeSessionResponse, SessionId, SessionNotification, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse,
    SetSessionModelRequest, SetSessionModelResponse,
};
use opentelemetry::metrics::Meter;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use trogon_std::time::GetElapsed;

use super::{
    authenticate, cancel, close_session, ext_method, ext_notification, fork_session, initialize,
    list_sessions, load_session, new_session, prompt, resume_session, set_session_config_option,
    set_session_mode, set_session_model,
};

/// Delay before publishing `session.ready` to NATS.
///
/// The `Agent` trait returns the response value *before* the transport layer
/// serializes and writes it to the client. Without a delay the spawned task
/// could publish `session.ready` before the client has received the
/// `session/new` response, violating the ordering guarantee.
const SESSION_READY_DELAY: Duration = Duration::from_millis(100);

pub struct Bridge<N, C: GetElapsed> {
    pub(crate) nats: N,
    pub(crate) clock: C,
    pub(crate) config: Config,
    pub(crate) metrics: Metrics,
    pub(crate) notification_sender: mpsc::Sender<SessionNotification>,
    pub(crate) background_tasks: RefCell<Vec<JoinHandle<()>>>,
}

impl<N, C: GetElapsed> Bridge<N, C> {
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
            background_tasks: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }

    pub(crate) fn spawn_background(&self, task: JoinHandle<()>) {
        self.background_tasks.borrow_mut().push(task);
    }

    pub async fn drain_background_tasks(&self) {
        let tasks: Vec<_> = self.background_tasks.borrow_mut().drain(..).collect();
        for task in tasks {
            let _ = task.await;
        }
    }
}

impl<N: PublishClient + FlushClient + Clone + Send + 'static, C: GetElapsed> Bridge<N, C> {
    pub(crate) fn schedule_session_ready(&self, session_id: SessionId) {
        let nats = self.nats.clone();
        let prefix = self.config.acp_prefix().to_string();
        let metrics = self.metrics.clone();
        let handle = tokio::spawn(async move {
            publish_session_ready(&nats, &prefix, &session_id, &metrics).await;
        });
        self.spawn_background(handle);
    }
}

async fn publish_session_ready<N: PublishClient + FlushClient>(
    nats: &N,
    prefix: &str,
    session_id: &SessionId,
    metrics: &Metrics,
) {
    tokio::time::sleep(SESSION_READY_DELAY).await;

    let subject = agent::ext_session_ready(prefix, &session_id.to_string());
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

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + SubscribeClient + FlushClient, C: GetElapsed> Agent
    for Bridge<N, C>
{
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        initialize::handle(self, args).await
    }

    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        authenticate::handle(self, args).await
    }

    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        new_session::handle(self, args).await
    }

    async fn load_session(&self, args: LoadSessionRequest) -> Result<LoadSessionResponse> {
        load_session::handle(self, args).await
    }

    async fn set_session_mode(
        &self,
        args: SetSessionModeRequest,
    ) -> Result<SetSessionModeResponse> {
        set_session_mode::handle(self, args).await
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        prompt::handle(self, args, &trogon_std::StdJsonSerialize).await
    }

    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        cancel::handle(self, args).await
    }

    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        list_sessions::handle(self, args).await
    }

    async fn set_session_config_option(
        &self,
        args: SetSessionConfigOptionRequest,
    ) -> Result<SetSessionConfigOptionResponse> {
        set_session_config_option::handle(self, args).await
    }

    async fn set_session_model(
        &self,
        args: SetSessionModelRequest,
    ) -> Result<SetSessionModelResponse> {
        set_session_model::handle(self, args).await
    }

    async fn fork_session(&self, args: ForkSessionRequest) -> Result<ForkSessionResponse> {
        fork_session::handle(self, args).await
    }

    async fn resume_session(&self, args: ResumeSessionRequest) -> Result<ResumeSessionResponse> {
        resume_session::handle(self, args).await
    }

    async fn close_session(&self, args: CloseSessionRequest) -> Result<CloseSessionResponse> {
        close_session::handle(self, args).await
    }

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        ext_method::handle(self, args).await
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        ext_notification::handle(self, args).await
    }
}
