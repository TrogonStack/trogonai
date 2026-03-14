// Safety: `CancelledSessions` uses `Mutex` for interior mutability so the `Bridge` remains
// `Send`/`Sync`. The fire-and-forget publish in `spawn_session_ready` uses `tokio::spawn`
// and captures only cloned, `Send` values — it never touches shared state from the closure.

mod authenticate;
mod cancel;
mod ext_method;
mod ext_notification;
mod initialize;
mod load_session;
mod new_session;
mod prompt;
mod set_session_mode;

use crate::config::{Config, SESSION_READY_DELAY};
use crate::nats::{self, ExtSessionReady, FlushClient, PublishClient, RequestClient, agent};
use crate::pending_prompt_waiters::PendingSessionPromptResponseWaiters;
use crate::prompt_slot_counter::PromptSlotCounter;
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::{
    Agent, AuthenticateRequest, AuthenticateResponse, CancelNotification, ExtNotification,
    ExtRequest, ExtResponse, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    Result, SessionId, SetSessionModeRequest, SetSessionModeResponse,
};
use opentelemetry::metrics::Meter;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{info, warn};
use trogon_std::time::GetElapsed;

const CANCELLED_SESSION_TTL: Duration = Duration::from_secs(300);
const CLEANUP_EVERY: usize = 16;

pub(crate) struct CancelledSessions<I: Copy> {
    map: Mutex<HashMap<SessionId, I>>,
    cleanup_counter: std::sync::atomic::AtomicUsize,
}

impl<I: Copy> CancelledSessions<I> {
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            cleanup_counter: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn mark_cancelled<C: GetElapsed<Instant = I>>(&self, session_id: SessionId, clock: &C) {
        let mut map = self.map.lock().unwrap();
        map.insert(session_id, clock.now());
        let count = self
            .cleanup_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if count.is_multiple_of(CLEANUP_EVERY) {
            map.retain(|_, ts| clock.elapsed(*ts) < CANCELLED_SESSION_TTL);
        }
    }

    pub fn take_if_cancelled<C: GetElapsed<Instant = I>>(
        &self,
        session_id: &SessionId,
        clock: &C,
    ) -> Option<()> {
        let mut map = self.map.lock().unwrap();
        let is_valid = map
            .get(session_id)
            .is_some_and(|ts| clock.elapsed(*ts) < CANCELLED_SESSION_TTL);

        if is_valid {
            map.remove(session_id);
            Some(())
        } else {
            map.remove(session_id);
            None
        }
    }
}

pub struct Bridge<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> {
    pub(crate) nats: N,
    pub(crate) clock: C,
    pub(crate) metrics: Metrics,
    pub(crate) cancelled_sessions: CancelledSessions<C::Instant>,
    pub(crate) pending_session_prompt_responses: PendingSessionPromptResponseWaiters<C::Instant>,
    pub(crate) prompt_slot_counter: PromptSlotCounter,
    pub(crate) config: Config,
    pub(crate) session_ready_publish_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Bridge<N, C> {
    pub fn new(nats: N, clock: C, meter: &Meter, config: Config) -> Self {
        let max_concurrent = config.max_concurrent_client_tasks();
        Self {
            nats,
            clock,
            config,
            metrics: Metrics::new(meter),
            cancelled_sessions: CancelledSessions::new(),
            pending_session_prompt_responses: PendingSessionPromptResponseWaiters::new(),
            prompt_slot_counter: PromptSlotCounter::new(max_concurrent),
            session_ready_publish_tasks: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn nats(&self) -> &N {
        &self.nats
    }

    pub(crate) fn register_session_ready_task(&self, task: tokio::task::JoinHandle<()>) {
        let mut tasks = self.session_ready_publish_tasks.lock().unwrap();
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
    }

    pub fn has_pending_session_ready_tasks(&self) -> bool {
        let mut tasks = self.session_ready_publish_tasks.lock().unwrap();
        tasks.retain(|task| !task.is_finished());
        !tasks.is_empty()
    }

    pub async fn await_session_ready_tasks(&self) {
        let tasks = std::mem::take(&mut *self.session_ready_publish_tasks.lock().unwrap());
        for task in tasks {
            if let Err(e) = task.await {
                warn!(error = %e, "session_ready task panicked");
            }
        }
    }

    pub(crate) fn spawn_session_ready(&self, session_id: &SessionId) {
        let nats_clone = self.nats.clone();
        let prefix = self.config.acp_prefix().to_string();
        let session_id = session_id.clone();
        let metrics = self.metrics.clone();
        let session_ready_task = tokio::spawn(async move {
            tokio::time::sleep(SESSION_READY_DELAY).await;

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
impl<N: RequestClient + PublishClient + FlushClient, C: GetElapsed> Agent for Bridge<N, C> {
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
mod send_sync_tests {
    use super::Bridge;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_std::time::SystemClock;

    #[test]
    fn bridge_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Bridge<AdvancedMockNatsClient, SystemClock>>();
    }
}

#[cfg(test)]
mod bridge_session_ready_tests {
    use super::*;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_std::time::MockClock;

    fn test_bridge() -> Bridge<AdvancedMockNatsClient, MockClock> {
        let nats = AdvancedMockNatsClient::new();
        let clock = MockClock::new();
        let provider = opentelemetry::global::meter_provider();
        let meter = provider.meter("test");
        let config = Config::for_test("acp");
        Bridge::new(nats, clock, &meter, config)
    }

    #[tokio::test]
    async fn register_and_has_pending() {
        let bridge = test_bridge();
        assert!(!bridge.has_pending_session_ready_tasks());

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = rx.await;
        });
        bridge.register_session_ready_task(task);
        assert!(bridge.has_pending_session_ready_tasks());
        tx.send(()).unwrap();
        bridge.await_session_ready_tasks().await;
    }

    #[tokio::test]
    async fn register_retains_only_unfinished_tasks() {
        let bridge = test_bridge();
        let finished = tokio::spawn(async {});
        bridge.register_session_ready_task(finished);
        tokio::task::yield_now().await;

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let pending = tokio::spawn(async move {
            let _ = rx.await;
        });
        bridge.register_session_ready_task(pending);

        {
            let tasks = bridge.session_ready_publish_tasks.lock().unwrap();
            assert_eq!(tasks.len(), 1);
        }

        tx.send(()).unwrap();
        bridge.await_session_ready_tasks().await;
    }

    #[tokio::test]
    async fn await_session_ready_tasks_drains() {
        let bridge = test_bridge();
        let task = tokio::spawn(async {});
        bridge.register_session_ready_task(task);
        bridge.await_session_ready_tasks().await;
        assert!(!bridge.has_pending_session_ready_tasks());
    }

    #[tokio::test]
    async fn await_handles_panicking_task() {
        let bridge = test_bridge();
        let task = tokio::spawn(async { panic!("intentional") });
        bridge.register_session_ready_task(task);
        bridge.await_session_ready_tasks().await;
        assert!(!bridge.has_pending_session_ready_tasks());
    }

    #[tokio::test]
    async fn spawn_session_ready_registers_task() {
        let bridge = test_bridge();
        let session_id = SessionId::new("test-session".to_string());
        bridge.spawn_session_ready(&session_id);

        assert!(bridge.has_pending_session_ready_tasks());
        bridge.await_session_ready_tasks().await;
    }

    #[tokio::test]
    async fn spawn_session_ready_error_path() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_publish_count(4);
        let clock = MockClock::new();
        let provider = opentelemetry::global::meter_provider();
        let meter = provider.meter("test");
        let config = Config::for_test("acp");
        let bridge = Bridge::new(nats, clock, &meter, config);

        let session_id = SessionId::new("fail-session".to_string());
        bridge.spawn_session_ready(&session_id);
        bridge.await_session_ready_tasks().await;
    }

    #[tokio::test]
    async fn has_pending_filters_finished_tasks() {
        let bridge = test_bridge();
        let task = tokio::spawn(async {});
        bridge.register_session_ready_task(task);
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(!bridge.has_pending_session_ready_tasks());
    }
}

#[cfg(test)]
mod cancelled_sessions_tests {
    use super::*;
    use agent_client_protocol::SessionId;
    use trogon_std::time::MockClock;

    fn session(id: &str) -> SessionId {
        SessionId::new(id.to_string())
    }

    #[test]
    fn mark_and_take_within_ttl() {
        let clock = MockClock::new();
        let cs = CancelledSessions::new();
        cs.mark_cancelled(session("s1"), &clock);
        assert!(cs.take_if_cancelled(&session("s1"), &clock).is_some());
    }

    #[test]
    fn take_removes_entry() {
        let clock = MockClock::new();
        let cs = CancelledSessions::new();
        cs.mark_cancelled(session("s1"), &clock);
        cs.take_if_cancelled(&session("s1"), &clock);
        assert!(cs.take_if_cancelled(&session("s1"), &clock).is_none());
    }

    #[test]
    fn take_returns_none_for_unknown_session() {
        let clock = MockClock::new();
        let cs = CancelledSessions::new();
        assert!(cs.take_if_cancelled(&session("nope"), &clock).is_none());
    }

    #[test]
    fn expired_entry_returns_none() {
        let clock = MockClock::new();
        let cs = CancelledSessions::new();
        cs.mark_cancelled(session("s1"), &clock);
        clock.advance(CANCELLED_SESSION_TTL + Duration::from_secs(1));
        assert!(cs.take_if_cancelled(&session("s1"), &clock).is_none());
    }

    #[test]
    fn cleanup_evicts_expired_entries() {
        let clock = MockClock::new();
        let cs = CancelledSessions::new();

        cs.mark_cancelled(session("old"), &clock);
        clock.advance(CANCELLED_SESSION_TTL + Duration::from_secs(1));

        for i in 0..CLEANUP_EVERY {
            cs.mark_cancelled(session(&format!("s{i}")), &clock);
        }

        let map = cs.map.lock().unwrap();
        assert!(!map.contains_key(&session("old")));
    }
}
