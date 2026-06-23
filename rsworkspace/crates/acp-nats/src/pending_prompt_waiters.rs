use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use agent_client_protocol::{PromptResponse, SessionId};
use tokio::sync::oneshot;
use trogon_std::time::GetElapsed;

use crate::constants::PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW;

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub(crate) struct PromptToken(pub u64);

#[derive(Debug, thiserror::Error)]
#[error("mutex lock poisoned")]
pub(crate) struct LockPoisonedError;

impl<T> From<PoisonError<T>> for LockPoisonedError {
    fn from(_: PoisonError<T>) -> Self {
        Self
    }
}

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

    pub(crate) fn purge_expired_timed_out_waiters<C: GetElapsed<Instant = I>>(
        &self,
        clock: &C,
    ) -> Result<(), LockPoisonedError> {
        self.timed_out
            .lock()?
            .retain(|_, seen_at| clock.elapsed(*seen_at) < PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW);
        Ok(())
    }

    pub(crate) fn should_suppress_missing_waiter_warning<C: GetElapsed<Instant = I>>(
        &self,
        session_id: &SessionId,
        prompt_token: PromptToken,
        _clock: &C,
    ) -> Result<bool, LockPoisonedError> {
        Ok(self.timed_out.lock()?.contains_key(&(session_id.clone(), prompt_token)))
    }

    pub fn resolve_waiter(
        &self,
        session_id: &SessionId,
        prompt_token: PromptToken,
        response: std::result::Result<PromptResponse, String>,
    ) -> Result<bool, LockPoisonedError> {
        let mut waiters = self.waiters.lock()?;
        let should_remove = waiters.get(session_id).is_some_and(|e| e.token == prompt_token);
        let waiter = if should_remove {
            waiters.remove(session_id)
        } else {
            None
        };
        drop(waiters);
        if let Some(waiter) = waiter {
            self.timed_out.lock()?.remove(&(session_id.clone(), prompt_token));
            Ok(waiter.sender.send(response).is_ok())
        } else {
            Ok(false)
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
mod tests;
