//! Coordinates session/prompt request-response cycle via NATS publish-subscribe.
//!
//! When a `session/prompt` request is published, we store the sender and await the backend's
//! `client.ext.session.prompt_response` notification.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use agent_client_protocol::{PromptResponse, SessionId};
use tokio::sync::oneshot;
use trogon_std::time::GetElapsed;

const PROMPT_TIMEOUT_WARNING_SUPPRESSION_SECS: u64 = 5;

type PromptResponseReceiver = oneshot::Receiver<std::result::Result<PromptResponse, String>>;

/// RAII guard that removes the registered waiter on drop.
/// Ensures cleanup when the prompt future is cancelled (e.g., client disconnect, task abort).
pub(crate) struct PromptWaiterGuard<'a, I: Copy> {
    waiters: &'a PendingSessionPromptResponseWaiters<I>,
    session_id: SessionId,
}

impl<'a, I: Copy> PromptWaiterGuard<'a, I> {
    fn new(waiters: &'a PendingSessionPromptResponseWaiters<I>, session_id: SessionId) -> Self {
        Self {
            waiters,
            session_id,
        }
    }
}

impl<'a, I: Copy> Drop for PromptWaiterGuard<'a, I> {
    fn drop(&mut self) {
        self.waiters.remove_waiter(&self.session_id);
    }
}

/// Coordinates session/prompt request-response cycle via NATS publish-subscribe.
/// When a `session/prompt` request is published, we store the sender and await the backend's
/// `client.ext.session.prompt_response` notification.
pub(crate) struct PendingSessionPromptResponseWaiters<I: Copy> {
    waiters:
        Mutex<HashMap<SessionId, oneshot::Sender<std::result::Result<PromptResponse, String>>>>,
    timed_out: Mutex<HashMap<SessionId, I>>,
}

impl<I: Copy> PendingSessionPromptResponseWaiters<I> {
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            timed_out: Mutex::new(HashMap::new()),
        }
    }

    pub fn register_waiter(
        &self,
        session_id: SessionId,
    ) -> std::result::Result<(PromptResponseReceiver, PromptWaiterGuard<'_, I>), ()> {
        let (tx, rx) = oneshot::channel();
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.contains_key(&session_id) {
            return Err(());
        }
        self.timed_out.lock().unwrap().remove(&session_id);
        waiters.insert(session_id.clone(), tx);
        Ok((rx, PromptWaiterGuard::new(self, session_id)))
    }

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

    pub(crate) fn purge_expired_timed_out_waiters<C: GetElapsed<Instant = I>>(&self, clock: &C) {
        self.timed_out.lock().unwrap().retain(|_, seen_at| {
            clock.elapsed(*seen_at) < Duration::from_secs(PROMPT_TIMEOUT_WARNING_SUPPRESSION_SECS)
        });
    }

    /// Delivers a prompt response to the waiting prompt caller.
    /// Call when the NATS subscriber receives a `client.ext.session.prompt_response` notification.
    #[allow(dead_code)]
    pub fn resolve_waiter(
        &self,
        session_id: &SessionId,
        response: std::result::Result<PromptResponse, String>,
    ) -> bool {
        let sender = self.waiters.lock().unwrap().remove(session_id);
        self.timed_out.lock().unwrap().remove(session_id);
        if let Some(sender) = sender {
            sender.send(response).is_ok()
        } else {
            false
        }
    }

    pub fn remove_waiter(&self, session_id: &SessionId) {
        self.waiters.lock().unwrap().remove(session_id);
    }
}
