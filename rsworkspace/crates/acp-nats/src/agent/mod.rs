mod authenticate;
mod cancel;
mod ext_method;
mod ext_notification;
mod initialize;
mod load_session;
mod new_session;
mod prompt;
mod set_session_mode;

use crate::metrics::Metrics;
use crate::nats::{FlushClient, PublishClient, RequestClient, SubscribeClient};
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SessionId, SetSessionModeRequest, SetSessionModeResponse,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use tokio::sync::oneshot;

/// Pre-flight checks to avoid sending prompt requests for cancelled sessions.
#[derive(Clone)]
pub(crate) struct CancelledSessions(Rc<RefCell<HashSet<SessionId>>>);

impl CancelledSessions {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(HashSet::new())))
    }

    pub fn is_cancelled(&self, session_id: &SessionId) -> bool {
        self.0.borrow().contains(session_id)
    }

    pub fn mark_cancelled(&self, session_id: SessionId) {
        self.0.borrow_mut().insert(session_id);
    }

    pub fn clear_cancellation(&self, session_id: &SessionId) -> bool {
        self.0.borrow_mut().remove(session_id)
    }
}

/// Coordinates session/prompt request-response cycle via NATS publish-subscribe.
/// When a `session/prompt` request is published, we store the sender and await the backend's
/// `client.ext.session.prompt_response` notification.
#[derive(Clone)]
pub(crate) struct PendingSessionPromptResponseWaiters(
    Rc<RefCell<HashMap<SessionId, oneshot::Sender<PromptResponse>>>>,
);

impl PendingSessionPromptResponseWaiters {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(HashMap::new())))
    }

    pub fn register_waiter(&self, session_id: SessionId) -> oneshot::Receiver<PromptResponse> {
        let (tx, rx) = oneshot::channel();
        self.0.borrow_mut().insert(session_id, tx);
        rx
    }

    pub fn resolve_waiter(&self, session_id: &SessionId, response: PromptResponse) -> bool {
        if let Some(sender) = self.0.borrow_mut().remove(session_id) {
            sender.send(response).is_ok()
        } else {
            false
        }
    }

    pub fn remove_waiter(&self, session_id: &SessionId) -> bool {
        self.0.borrow_mut().remove(session_id).is_some()
    }
}

pub struct Bridge<N: SubscribeClient + RequestClient + PublishClient + FlushClient> {
    pub(crate) nats: Option<N>,
    pub(crate) metrics: Metrics,
    pub(crate) cancelled_sessions: CancelledSessions,
    pub(crate) pending_session_prompt_responses: PendingSessionPromptResponseWaiters,
    pub(crate) acp_prefix: String,
}

impl<N: SubscribeClient + RequestClient + PublishClient + FlushClient> Bridge<N> {
    pub fn new(nats: Option<N>, acp_prefix: String) -> Self {
        Self {
            nats,
            metrics: Metrics::new(),
            cancelled_sessions: CancelledSessions::new(),
            pending_session_prompt_responses: PendingSessionPromptResponseWaiters::new(),
            acp_prefix,
        }
    }

    pub(crate) fn require_nats(&self) -> Result<&N> {
        self.nats.as_ref().ok_or_else(|| {
            self.metrics.record_error("nats_unavailable");
            Error::new(
                -32000,
                "NATS connection unavailable - bridge cannot function without NATS",
            )
        })
    }
}

#[async_trait::async_trait(?Send)]
impl<N: SubscribeClient + RequestClient + PublishClient + FlushClient> Agent for Bridge<N> {
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
        prompt::handle(self, args).await
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
