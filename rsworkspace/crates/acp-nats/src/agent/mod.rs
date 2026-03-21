mod authenticate;
mod cancel;
mod ext_method;
mod ext_notification;
mod initialize;
mod load_session;
mod new_session;
mod prompt;
mod set_session_mode;

pub use prompt::REQ_ID_HEADER;

use std::time::Duration;

use crate::config::Config;
use crate::nats::{
    self, ExtSessionReady, FlushClient, FlushPolicy, PublishClient, PublishOptions, RequestClient,
    RetryPolicy, SubscribeClient, agent,
};
use crate::pending_prompt_waiters::PendingSessionPromptResponseWaiters;
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SessionId, SessionNotification, SetSessionModeRequest, SetSessionModeResponse,
};
use opentelemetry::metrics::Meter;
use std::cell::RefCell;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};
use trogon_std::time::GetElapsed;

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
    pub(crate) pending_session_prompt_responses: PendingSessionPromptResponseWaiters<C::Instant>,
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
            pending_session_prompt_responses: PendingSessionPromptResponseWaiters::new(),
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

    async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse> {
        ext_method::handle(self, args).await
    }

    async fn ext_notification(&self, args: ExtNotification) -> Result<()> {
        ext_notification::handle(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Bridge;
    use crate::config::Config;
    use agent_client_protocol::{
        Agent, ExtNotification, ExtRequest, PromptRequest, PromptResponse, SessionNotification,
        StopReason,
    };
    use tokio::sync::mpsc;
    use trogon_nats::AdvancedMockNatsClient;

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let (tx, _rx) = mpsc::channel::<SessionNotification>(64);
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
            tx,
        );
        (mock, bridge)
    }

    fn empty_raw_value() -> Arc<serde_json::value::RawValue> {
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap())
    }

    #[tokio::test]
    async fn drain_background_tasks_completes() {
        let (_mock, bridge) = mock_bridge();
        bridge.spawn_background(tokio::spawn(async {}));
        bridge.drain_background_tasks().await;
        assert!(bridge.background_tasks.borrow().is_empty());
    }

    #[tokio::test]
    async fn prompt_via_agent_trait_returns_done() {
        let (mock, bridge) = mock_bridge();

        let _notif_tx = mock.inject_messages();
        let resp_tx = mock.inject_messages();
        let _cancel_tx = mock.inject_messages();

        let response = PromptResponse::new(StopReason::EndTurn);
        resp_tx
            .unbounded_send(async_nats::Message {
                subject: "test".into(),
                reply: None,
                payload: bytes::Bytes::from(serde_json::to_vec(&response).unwrap()),
                headers: None,
                status: None,
                description: None,
                length: 0,
            })
            .unwrap();

        let result = bridge.prompt(PromptRequest::new("s1", vec![])).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn ext_method_returns_agent_unavailable_when_nats_fails() {
        use agent_client_protocol::ErrorCode;

        let (_mock, bridge) = mock_bridge();
        let err = bridge
            .ext_method(ExtRequest::new("ext", empty_raw_value()))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Other(crate::error::AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn ext_notification_returns_ok_even_when_publish_fails() {
        let (_mock, bridge) = mock_bridge();
        assert!(
            bridge
                .ext_notification(ExtNotification::new("ext", empty_raw_value()))
                .await
                .is_ok()
        );
    }
}
