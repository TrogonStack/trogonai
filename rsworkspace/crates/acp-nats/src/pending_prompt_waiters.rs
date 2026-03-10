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
//! - Per-prompt correlation via `PromptToken` prevents late responses for prompt A from resolving
//!   a newly registered prompt B for the same session.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW;
use agent_client_protocol::{PromptResponse, SessionId};
use tokio::sync::oneshot;
use trogon_std::time::GetElapsed;

type PromptResponseReceiver = oneshot::Receiver<std::result::Result<PromptResponse, String>>;

/// Per-prompt correlation token. Ensures late responses for prompt A cannot resolve prompt B.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub(crate) struct PromptToken(pub u64);

struct WaiterEntry {
    token: PromptToken,
    sender: oneshot::Sender<std::result::Result<PromptResponse, String>>,
}

/// Lifetime token for a registered session waiter.
///
/// Dropping the guard removes the waiter so cancellations and task aborts do not leak entries.
pub(crate) struct PromptWaiterGuard<'a, I: Copy> {
    waiters: &'a PendingSessionPromptResponseWaiters<I>,
    session_id: SessionId,
    prompt_token: PromptToken,
}

impl<'a, I: Copy> PromptWaiterGuard<'a, I> {
    fn new(
        waiters: &'a PendingSessionPromptResponseWaiters<I>,
        session_id: SessionId,
        prompt_token: PromptToken,
    ) -> Self {
        Self {
            waiters,
            session_id,
            prompt_token,
        }
    }
}

impl<'a, I: Copy> Drop for PromptWaiterGuard<'a, I> {
    fn drop(&mut self) {
        self.waiters
            .remove_waiter_if_token_matches(&self.session_id, self.prompt_token);
    }
}

/// Process-local map of in-flight prompt waiters keyed by session.
///
/// Scope is intentionally local to this agent process; cross-process correlation belongs to NATS
/// subjects and backend state.
pub(crate) struct PendingSessionPromptResponseWaiters<I: Copy> {
    waiters: Mutex<HashMap<SessionId, WaiterEntry>>,
    next_waiter_token: AtomicU64,
    timed_out: Mutex<HashMap<(SessionId, PromptToken), I>>,
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
    /// Returns `(receiver, guard, prompt_token)`; the token must be sent with the request and
    /// echoed in the response for correct correlation.
    pub fn register_waiter(
        &self,
        session_id: SessionId,
    ) -> std::result::Result<
        (
            PromptResponseReceiver,
            PromptWaiterGuard<'_, I>,
            PromptToken,
        ),
        (),
    > {
        let (tx, rx) = oneshot::channel();
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.contains_key(&session_id) {
            return Err(());
        }
        let token_value = self.next_waiter_token.fetch_add(1, Ordering::Relaxed);
        let prompt_token = PromptToken(token_value);
        self.timed_out
            .lock()
            .unwrap()
            .retain(|(s, _), _| s != &session_id);
        waiters.insert(
            session_id.clone(),
            WaiterEntry {
                token: prompt_token,
                sender: tx,
            },
        );
        Ok((
            rx,
            PromptWaiterGuard::new(self, session_id, prompt_token),
            prompt_token,
        ))
    }

    /// Marks a prompt waiter as timed out to suppress transient duplicate warnings for late responses.
    pub(crate) fn mark_prompt_waiter_timed_out<C: GetElapsed<Instant = I>>(
        &self,
        session_id: SessionId,
        prompt_token: PromptToken,
        clock: &C,
    ) {
        self.purge_expired_timed_out_waiters(clock);
        self.timed_out
            .lock()
            .unwrap()
            .insert((session_id, prompt_token), clock.now());
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

    /// Returns true if a late prompt response for this (session, token) should not emit a missing-waiter warning.
    pub(crate) fn should_suppress_missing_waiter_warning<C: GetElapsed<Instant = I>>(
        &self,
        session_id: &SessionId,
        prompt_token: PromptToken,
        _clock: &C,
    ) -> bool {
        self.timed_out
            .lock()
            .unwrap()
            .contains_key(&(session_id.clone(), prompt_token))
    }

    /// Delivers a backend prompt result to the waiting caller for `(session_id, prompt_token)`.
    /// Only resolves if the token matches; late responses for a different prompt are ignored.
    pub fn resolve_waiter(
        &self,
        session_id: &SessionId,
        prompt_token: PromptToken,
        response: std::result::Result<PromptResponse, String>,
    ) -> bool {
        let mut waiters = self.waiters.lock().unwrap();
        let should_remove = waiters
            .get(session_id)
            .is_some_and(|e| e.token == prompt_token);
        let waiter = if should_remove {
            waiters.remove(session_id)
        } else {
            None
        };
        drop(waiters);
        if let Some(waiter) = waiter {
            self.timed_out
                .lock()
                .unwrap()
                .remove(&(session_id.clone(), prompt_token));
            waiter.sender.send(response).is_ok()
        } else {
            false
        }
    }

    fn remove_waiter_if_token_matches(&self, session_id: &SessionId, prompt_token: PromptToken) {
        let mut waiters = self.waiters.lock().unwrap();
        if waiters
            .get(session_id)
            .is_some_and(|entry| entry.token == prompt_token)
        {
            waiters.remove(session_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn has_waiter(&self, session_id: &SessionId) -> bool {
        self.waiters.lock().unwrap().contains_key(session_id)
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
            PromptToken(0),
            Ok(PromptResponse::new(StopReason::EndTurn)),
        );
        assert!(!resolved);
    }

    #[test]
    fn purge_expired_timed_out_waiters_removes_expired_markers() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let clock = MockClock::new();
        waiters.mark_prompt_waiter_timed_out(SessionId::from("s1"), PromptToken(0), &clock);
        assert_eq!(waiters.timed_out.lock().unwrap().len(), 1);

        clock.advance(PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW + Duration::from_millis(1));
        waiters.purge_expired_timed_out_waiters(&clock);

        assert!(waiters.timed_out.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dropping_old_guard_does_not_remove_new_waiter_for_same_session() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let session_id = SessionId::from("s1");

        let (_rx1, guard1, token1) = waiters.register_waiter(session_id.clone()).unwrap();
        assert!(waiters.resolve_waiter(
            &session_id,
            token1,
            Ok(PromptResponse::new(StopReason::EndTurn))
        ));

        let (rx2, _guard2, token2) = waiters.register_waiter(session_id.clone()).unwrap();

        drop(guard1);

        assert!(
            waiters.resolve_waiter(
                &session_id,
                token2,
                Ok(PromptResponse::new(StopReason::EndTurn))
            ),
            "old guard must not remove the new waiter's sender"
        );

        let response = rx2
            .await
            .expect("new waiter should remain connected")
            .expect("new waiter should receive a valid response");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
    }
}
