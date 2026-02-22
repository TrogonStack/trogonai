// Safety: `CancelledSessions` and `PendingSessionPromptResponseWaiters` use `RefCell`
// for interior mutability. This is sound because the `Bridge` must be driven from a
// single task (or a single-threaded `LocalSet`). The fire-and-forget publish in
// `spawn_session_ready` uses `tokio::spawn` and captures only cloned, `Send` values —
// it never touches the `RefCell` fields.

mod authenticate;
mod cancel;
mod error;
mod ext_method;
mod ext_notification;
mod initialize;
mod load_session;
mod new_session;
mod prompt;
mod set_session_mode;

use crate::config::Config;
use crate::session_id::AcpSessionId;
use crate::nats::{
    self, ExtSessionReady, FlushClient, PublishClient, RequestClient, agent,
};
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, Error, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SessionId, SetSessionModeRequest, SetSessionModeResponse,
};
use opentelemetry::metrics::Meter;
use std::marker::PhantomData;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{info, warn};
use trogon_std::time::GetElapsed;

const CANCELLED_SESSION_TTL: Duration = Duration::from_secs(300); // 5 minutes
const CLEANUP_EVERY: usize = 16;
const PROMPT_TIMEOUT_WARNING_SUPPRESSION_SECS: u64 = 5;

/// Pre-flight checks to avoid sending prompt requests for cancelled sessions.
///
/// Entries are evicted after `CANCELLED_SESSION_TTL` to prevent unbounded growth
/// when sessions are cancelled but never receive a subsequent prompt.
pub(crate) struct CancelledSessions<I: Copy> {
    map: RefCell<HashMap<SessionId, I>>,
    cleanup_counter: Cell<usize>,
}

impl<I: Copy> CancelledSessions<I> {
    pub fn new() -> Self {
        Self {
            map: RefCell::new(HashMap::new()),
            cleanup_counter: Cell::new(0),
        }
    }

    pub fn mark_cancelled<C: GetElapsed<Instant = I>>(&self, session_id: SessionId, clock: &C) {
        let mut map = self.map.borrow_mut();
        map.insert(session_id, clock.now());
        let cleanup_due_count = self.cleanup_counter.get().wrapping_add(1);
        self.cleanup_counter.set(cleanup_due_count);
        if cleanup_due_count % CLEANUP_EVERY == 0 {
            map.retain(|_, ts| clock.elapsed(*ts) < CANCELLED_SESSION_TTL);
        }
    }

    /// Atomically checks if a session is cancelled and clears it if so.
    ///
    /// Returns `Some(())` if the session was cancelled (and has now been cleared),
    /// or `None` if the session was not found or has expired.
    /// `None` therefore also means "not cancelled", because the method does not
    /// expose expiration state separately.
    pub fn take_if_cancelled<C: GetElapsed<Instant = I>>(
        &self,
        session_id: &SessionId,
        clock: &C,
    ) -> Option<()> {
        let mut map = self.map.borrow_mut();

        let is_valid = map
            .get(session_id)
            .is_some_and(|ts| clock.elapsed(*ts) < CANCELLED_SESSION_TTL);

        if is_valid {
            map.remove(session_id);
            Some(())
        } else {
            // Clean up expired entry if present
            map.remove(session_id);
            None
        }
    }
}

/// Coordinates session/prompt request-response cycle via NATS publish-subscribe.
/// When a `session/prompt` request is published, we store the sender and await the backend's
/// `client.ext.session.prompt_response` notification.
pub(crate) struct PendingSessionPromptResponseWaiters<I: Copy> {
    waiters: RefCell<HashMap<SessionId, oneshot::Sender<Result<PromptResponse, String>>>>,
    timed_out: RefCell<HashMap<SessionId, I>>,
}

impl<I: Copy> PendingSessionPromptResponseWaiters<I> {
    pub fn new() -> Self {
        Self {
            waiters: RefCell::new(HashMap::new()),
            timed_out: RefCell::new(HashMap::new()),
        }
    }

    /// Registers one waiter per session. If a waiter already exists for a session,
    /// registration fails to avoid overwriting an in-flight prompt request.
    pub fn register_waiter(
        &self,
        session_id: SessionId,
    ) -> Result<oneshot::Receiver<Result<PromptResponse, String>>, ()> {
        let mut waiters = self.waiters.borrow_mut();
        if waiters.contains_key(&session_id) {
            return Err(());
        }
        let (tx, rx) = oneshot::channel();
        self.timed_out.borrow_mut().remove(&session_id);
        waiters.insert(session_id, tx);
        Ok(rx)
    }

    #[allow(dead_code)] // Used by integration tests in src/tests/
    pub(crate) fn has_waiter(&self, session_id: &SessionId) -> bool {
        self.waiters.borrow().contains_key(session_id)
    }

    pub(crate) fn mark_prompt_waiter_timed_out<C: GetElapsed<Instant = I>>(
        &self,
        session_id: SessionId,
        clock: &C,
    ) {
        self.purge_expired_timed_out_waiters(clock);
        self.timed_out
            .borrow_mut()
            .insert(session_id, clock.now());
    }

    pub(crate) fn purge_expired_timed_out_waiters<C: GetElapsed<Instant = I>>(&self, clock: &C) {
        self.timed_out.borrow_mut().retain(|_, seen_at| {
            clock.elapsed(*seen_at) < Duration::from_secs(PROMPT_TIMEOUT_WARNING_SUPPRESSION_SECS)
        });
    }

    pub(crate) fn should_suppress_missing_waiter_warning<C: GetElapsed<Instant = I>>(
        &self,
        session_id: &SessionId,
        _clock: &C,
    ) -> bool {
        self.timed_out.borrow().contains_key(session_id)
    }

    pub fn resolve_waiter(
        &self,
        session_id: &SessionId,
        response: Result<PromptResponse, String>,
    ) -> bool {
        self.timed_out.borrow_mut().remove(session_id);
        if let Some(sender) = self.waiters.borrow_mut().remove(session_id) {
            sender.send(response).is_ok()
        } else {
            false
        }
    }

    pub fn remove_waiter(&self, session_id: &SessionId) {
        self.waiters.borrow_mut().remove(session_id);
    }
}

/// NATS-backed implementation of the Agent Client Protocol.
///
/// Translates ACP JSON-RPC calls into NATS request/reply messages and manages
/// the asynchronous prompt response lifecycle via publish-subscribe.
///
/// # Thread Safety
///
/// This type uses `RefCell` for interior mutability in `CancelledSessions` and
/// `PendingSessionPromptResponseWaiters`. The `Bridge` must be driven from a
/// **single task** (or a single-threaded `LocalSet`) so that no two `&self`
/// methods execute concurrently and race on the `RefCell` fields. A
/// `PhantomData<Rc<()>>` marker also prevents accidental `Send`/`Sync` use of
/// `Bridge`.
///
/// Background work (`spawn_session_ready`) is dispatched via `tokio::spawn`
/// and only captures cloned, `Send` data — it never touches the `RefCell`s.
/// Those task handles are now tracked so shutdown can await their completion.
pub struct Bridge<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> {
    pub(crate) nats: N,
    pub(crate) clock: C,
    pub(crate) metrics: Metrics,
    pub(crate) cancelled_sessions: CancelledSessions<C::Instant>,
    pub(crate) pending_session_prompt_responses: PendingSessionPromptResponseWaiters<C::Instant>,
    pub(crate) config: Config,
    _not_send_sync: PhantomData<std::rc::Rc<()>>,
    agent_prompt_requests_in_flight: Cell<usize>,
    pub(crate) session_ready_publish_tasks: RefCell<Vec<tokio::task::JoinHandle<()>>>,
}

impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Bridge<N, C> {
    pub fn new(nats: N, clock: C, meter: &Meter, config: Config) -> Self {
        Self {
            nats,
            clock,
            metrics: Metrics::new(meter),
            cancelled_sessions: CancelledSessions::new(),
            pending_session_prompt_responses: PendingSessionPromptResponseWaiters::new(),
            config,
            _not_send_sync: PhantomData,
            agent_prompt_requests_in_flight: Cell::new(0),
            session_ready_publish_tasks: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn register_session_ready_task(&self, task: tokio::task::JoinHandle<()>) {
        let mut tasks = self.session_ready_publish_tasks.borrow_mut();
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
    }

    pub fn has_pending_session_ready_tasks(&self) -> bool {
        !self.session_ready_publish_tasks.borrow().is_empty()
    }

    pub async fn await_session_ready_tasks(&self) {
        let tasks = std::mem::take(&mut *self.session_ready_publish_tasks.borrow_mut());
        for task in tasks {
            if let Err(e) = task.await {
                warn!(error = %e, "session_ready task panicked");
            }
        }
    }

    pub(crate) fn try_acquire_prompt_slot(&self) -> bool {
        // This counter is an independent cap from the client task backpressure counter
        // in `client::run` and intentionally shares the same configured limit.
        let max = self.config.max_concurrent_client_tasks();
        if self.agent_prompt_requests_in_flight.get() >= max {
            false
        } else {
            self.agent_prompt_requests_in_flight
                .set(self.agent_prompt_requests_in_flight.get() + 1);
            true
        }
    }

    pub(crate) fn release_prompt_slot(&self) {
        self.agent_prompt_requests_in_flight
            .set(self.agent_prompt_requests_in_flight.get().saturating_sub(1));
    }

    pub(crate) fn validate_session(&self, session_id: &SessionId) -> Result<AcpSessionId> {
        AcpSessionId::try_from(session_id).map_err(|e| {
            self.metrics.record_error("session.validate", "invalid_session_id");
            Error::new(
                ErrorCode::InvalidParams.into(),
                format!("Invalid session ID: {}", e),
            )
        })
    }

    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }
    pub(crate) fn spawn_session_ready(&self, session_id: &SessionId) {
        let nats_clone = self.nats.clone();
        let prefix = self.config.acp_prefix.clone();
        let session_id = session_id.clone();
        let metrics = self.metrics.clone();
        let session_ready_task = tokio::spawn(async move {
            let ready_subject = agent::ext_session_ready(&prefix, &session_id.to_string());
            info!(session_id = %session_id, subject = %ready_subject, "Publishing session.ready");

            let ready_message = ExtSessionReady::new(session_id.clone());

            let options = nats::PublishOptions::builder()
                .publish_retry_policy(nats::RetryPolicy::standard())
                .flush_policy(nats::FlushPolicy::standard())
                .build();

            if let Err(e) =
                nats::publish(&nats_clone, &ready_subject, &ready_message, options).await
            {
                warn!(
                    error = %e,
                    session_id = %session_id,
                    "Failed to publish session.ready"
                );
                metrics.record_error("session_ready", "session_ready_publish_failed");
            } else {
                info!(session_id = %session_id, "Published session.ready");
            }
        });
        self.register_session_ready_task(session_ready_task);
    }
}

#[async_trait::async_trait(?Send)]
impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Agent
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

#[cfg(test)]
mod tests {
    // All agent methods (set_session_mode, prompt, cancel, ext_method, ext_notification)
    // are now implemented. Stub behavior tests removed; see tests::agent_handlers for
    // integration coverage.
}
