//! Waiter registry for bridging prompt request/response over NATS notifications.
//!
//! **When to use**
//! - In the ACP prompt path where request and response are decoupled (`publish` now, response
//!   arrives later via `client.ext.session.prompt_response`).
//! - Before publishing prompt work, so an immediate backend response cannot race ahead of waiter
//!   registration.
//!
//! **Why this exists**
//! - Prompt responses are correlated by `SessionId`, not by direct request/reply transport.
//! - Enforcing one active waiter per session avoids ambiguous delivery when clients duplicate
//!   prompt calls.
//! - Timed-out sessions are tracked briefly to suppress noisy duplicate timeout-related warnings
//!   during late-response windows.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW;
use agent_client_protocol::{PromptResponse, SessionId};
use tokio::sync::oneshot;
use trogon_std::time::GetElapsed;

type PromptResponseReceiver = oneshot::Receiver<std::result::Result<PromptResponse, String>>;

struct WaiterEntry {
    token: u64,
    sender: oneshot::Sender<std::result::Result<PromptResponse, String>>,
}

/// Lifetime token for a registered session waiter.
///
/// Dropping the guard removes the waiter so cancellations and task aborts do not leak entries.
pub(crate) struct PromptWaiterGuard<'a, I: Copy> {
    waiters: &'a PendingSessionPromptResponseWaiters<I>,
    session_id: SessionId,
    waiter_token: u64,
}

impl<'a, I: Copy> PromptWaiterGuard<'a, I> {
    fn new(
        waiters: &'a PendingSessionPromptResponseWaiters<I>,
        session_id: SessionId,
        waiter_token: u64,
    ) -> Self {
        Self {
            waiters,
            session_id,
            waiter_token,
        }
    }
}

impl<'a, I: Copy> Drop for PromptWaiterGuard<'a, I> {
    fn drop(&mut self) {
        self.waiters
            .remove_waiter_if_token_matches(&self.session_id, self.waiter_token);
    }
}

/// Process-local map of in-flight prompt waiters keyed by session.
///
/// Scope is intentionally local to this agent process; cross-process correlation belongs to NATS
/// subjects and backend state.
pub(crate) struct PendingSessionPromptResponseWaiters<I: Copy> {
    waiters: Mutex<HashMap<SessionId, WaiterEntry>>,
    next_waiter_token: AtomicU64,
    timed_out: Mutex<HashMap<SessionId, I>>,
}

impl<I: Copy> PendingSessionPromptResponseWaiters<I> {
    /// Creates an empty waiter registry.
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            next_waiter_token: AtomicU64::new(0),
            timed_out: Mutex::new(HashMap::new()),
        }
    }

    /// Registers the receiver for the next prompt response of `session_id`.
    ///
    /// Returns `Err(())` when another waiter is already active for the same session.
    pub fn register_waiter(
        &self,
        session_id: SessionId,
    ) -> std::result::Result<(PromptResponseReceiver, PromptWaiterGuard<'_, I>), ()> {
        let (tx, rx) = oneshot::channel();
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.contains_key(&session_id) {
            return Err(());
        }
        let waiter_token = self.next_waiter_token.fetch_add(1, Ordering::Relaxed);
        self.timed_out.lock().unwrap().remove(&session_id);
        waiters.insert(
            session_id.clone(),
            WaiterEntry {
                token: waiter_token,
                sender: tx,
            },
        );
        Ok((rx, PromptWaiterGuard::new(self, session_id, waiter_token)))
    }

    /// Marks a session as timed out to suppress transient duplicate warnings for late responses.
    pub(crate) fn mark_prompt_waiter_timed_out<C: GetElapsed<Instant = I>>(
        &self,
        session_id: SessionId,
        clock: &C,
    ) {
        self.purge_expired_timed_out_waiters(clock);
        self.timed_out
            .lock()
            .unwrap()
            .insert(session_id, clock.now());
    }

    /// Drops timeout-suppression markers after a short window.
    ///
    /// This keeps suppression bounded so future requests for the same session can emit warnings
    /// again if they truly timeout.
    pub(crate) fn purge_expired_timed_out_waiters<C: GetElapsed<Instant = I>>(&self, clock: &C) {
        self.timed_out.lock().unwrap().retain(|_, seen_at| {
            clock.elapsed(*seen_at) < PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW
        });
    }

    /// Delivers a backend prompt result to the currently waiting caller for `session_id`.
    #[allow(dead_code)]
    pub fn resolve_waiter(
        &self,
        session_id: &SessionId,
        response: std::result::Result<PromptResponse, String>,
    ) -> bool {
        let waiter = self.waiters.lock().unwrap().remove(session_id);
        self.timed_out.lock().unwrap().remove(session_id);
        if let Some(waiter) = waiter {
            waiter.sender.send(response).is_ok()
        } else {
            false
        }
    }

    fn remove_waiter_if_token_matches(&self, session_id: &SessionId, waiter_token: u64) {
        let mut waiters = self.waiters.lock().unwrap();
        if waiters
            .get(session_id)
            .is_some_and(|entry| entry.token == waiter_token)
        {
            waiters.remove(session_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_waiter_for_test(&self, session_id: &SessionId) {
        self.waiters.lock().unwrap().remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_client_protocol::{PromptResponse, SessionId, StopReason};
    use trogon_std::time::{MockClock, MockInstant};

    use super::*;

    #[test]
    fn resolve_waiter_returns_false_when_no_waiter_registered() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let resolved = waiters.resolve_waiter(
            &SessionId::from("s1"),
            Ok(PromptResponse::new(StopReason::EndTurn)),
        );
        assert!(!resolved);
    }

    #[test]
    fn purge_expired_timed_out_waiters_removes_expired_markers() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let clock = MockClock::new();

        waiters.mark_prompt_waiter_timed_out(SessionId::from("s1"), &clock);
        assert_eq!(waiters.timed_out.lock().unwrap().len(), 1);

        clock.advance(PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW + Duration::from_millis(1));
        waiters.purge_expired_timed_out_waiters(&clock);

        assert!(waiters.timed_out.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dropping_old_guard_does_not_remove_new_waiter_for_same_session() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let session_id = SessionId::from("s1");

        let (_rx1, guard1) = waiters.register_waiter(session_id.clone()).unwrap();
        assert!(waiters.resolve_waiter(&session_id, Ok(PromptResponse::new(StopReason::EndTurn))));

        let (rx2, _guard2) = waiters.register_waiter(session_id.clone()).unwrap();

        drop(guard1);

        assert!(
            waiters.resolve_waiter(&session_id, Ok(PromptResponse::new(StopReason::EndTurn))),
            "old guard must not remove the new waiter's sender"
        );

        let response = rx2
            .await
            .expect("new waiter should remain connected")
            .expect("new waiter should receive a valid response");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
    }
}
