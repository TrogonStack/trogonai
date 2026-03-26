use std::collections::HashMap;
use std::sync::Mutex;

use agent_client_protocol::{PromptResponse, SessionId};
use tokio::sync::oneshot;
use trogon_std::time::GetElapsed;

use crate::constants::PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW;

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub(crate) struct PromptToken(pub u64);

struct WaiterEntry {
    token: PromptToken,
    sender: oneshot::Sender<std::result::Result<PromptResponse, String>>,
}

pub(crate) struct PendingSessionPromptResponseWaiters<I: Copy> {
    waiters: Mutex<HashMap<SessionId, WaiterEntry>>,
    timed_out: Mutex<HashMap<(SessionId, PromptToken), I>>,
}

impl<I: Copy> PendingSessionPromptResponseWaiters<I> {
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            timed_out: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn purge_expired_timed_out_waiters<C: GetElapsed<Instant = I>>(&self, clock: &C) {
        self.timed_out.lock().unwrap().retain(|_, seen_at| {
            clock.elapsed(*seen_at) < PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW
        });
    }

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

    #[cfg(test)]
    pub(crate) fn register_waiter(
        &self,
        session_id: SessionId,
    ) -> std::result::Result<
        (
            oneshot::Receiver<std::result::Result<PromptResponse, String>>,
            PromptToken,
        ),
        (),
    > {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);

        let (tx, rx) = oneshot::channel();
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.contains_key(&session_id) {
            return Err(());
        }
        let token = PromptToken(NEXT_TOKEN.fetch_add(1, Ordering::Relaxed));
        waiters.insert(session_id, WaiterEntry { token, sender: tx });
        Ok((rx, token))
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
    use trogon_std::time::{GetNow, MockClock, MockInstant};

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
    fn register_waiter_rejects_duplicate_session() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let session_id = SessionId::from("s1");
        let (_rx, _token) = waiters.register_waiter(session_id.clone()).unwrap();
        assert!(waiters.register_waiter(session_id).is_err());
    }

    #[test]
    fn purge_expired_timed_out_waiters_removes_expired_markers() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let clock = MockClock::new();
        {
            let mut timed_out = waiters.timed_out.lock().unwrap();
            timed_out.insert((SessionId::from("s1"), PromptToken(0)), clock.now());
        }
        assert_eq!(waiters.timed_out.lock().unwrap().len(), 1);

        clock.advance(PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW + Duration::from_millis(1));
        waiters.purge_expired_timed_out_waiters(&clock);

        assert!(waiters.timed_out.lock().unwrap().is_empty());
    }

    #[test]
    fn purge_keeps_non_expired_markers() {
        let waiters = PendingSessionPromptResponseWaiters::<MockInstant>::new();
        let clock = MockClock::new();
        let old_instant = clock.now();
        clock.advance(PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW + Duration::from_millis(1));
        let fresh_instant = clock.now();
        {
            let mut timed_out = waiters.timed_out.lock().unwrap();
            timed_out.insert((SessionId::from("old"), PromptToken(0)), old_instant);
            timed_out.insert((SessionId::from("fresh"), PromptToken(1)), fresh_instant);
        }
        assert_eq!(waiters.timed_out.lock().unwrap().len(), 2);

        waiters.purge_expired_timed_out_waiters(&clock);

        let timed_out = waiters.timed_out.lock().unwrap();
        assert_eq!(timed_out.len(), 1);
        assert!(timed_out.contains_key(&(SessionId::from("fresh"), PromptToken(1))));
    }
}
