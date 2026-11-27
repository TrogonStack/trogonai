use crate::constants::REINITIALIZE_TIMEOUT;
use agent_client_protocol::v1::schema::{InitializeRequest, InitializeResponse};
use agent_client_protocol::{Error, ErrorCode, Result};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

/// The claiming client's `initialize`, kept across child runs so that every
/// child the host launches is initialized the way that client asked:
/// its protocol version, `clientCapabilities` (with their `_meta`, which is
/// where an agent such as fx reads the host tools a client offers),
/// `clientInfo`, and the request's own `_meta`.
///
/// A run whose child is launched while a request is remembered initializes it
/// with that request before serving: a relaunch after a crash, or the relaunch
/// a client asks for by initializing again with different parameters. That
/// run reports how the child answered through [`Self::settle`], so the
/// client's `initialize` that asked for the relaunch can return the new
/// child's real response.
#[derive(Debug)]
pub struct ClientHandshake {
    last: Mutex<Option<InitializeRequest>>,
    outcomes: watch::Sender<Option<Arc<Outcome>>>,
}

#[derive(Debug)]
struct Outcome {
    request: InitializeRequest,
    result: Result<InitializeResponse>,
}

impl Default for ClientHandshake {
    fn default() -> Self {
        Self {
            last: Mutex::default(),
            outcomes: watch::Sender::new(None),
        }
    }
}

impl ClientHandshake {
    /// The `initialize` the next child is to be launched with, if a client has
    /// sent one the host still holds.
    pub fn last(&self) -> Option<InitializeRequest> {
        self.lock().clone()
    }

    /// Keeps `request` for every later child.
    pub fn remember(&self, request: InitializeRequest) {
        *self.lock() = Some(request);
    }

    /// Records how a freshly launched child answered `request`, for the
    /// client waiting on it. A refused request is dropped, so the next child
    /// is not launched into the same refusal: it waits for the client instead.
    pub fn settle(&self, request: &InitializeRequest, result: &Result<InitializeResponse>) {
        if result.is_err() {
            let mut last = self.lock();
            if last.as_ref() == Some(request) {
                *last = None;
            }
        }
        self.outcomes.send_replace(Some(Arc::new(Outcome {
            request: request.clone(),
            result: result.clone(),
        })));
    }

    /// Starts listening for children answering a request. Call it before
    /// asking for the relaunch, so its outcome cannot be missed; outcomes
    /// recorded before this call are not seen.
    pub fn listen(&self) -> PendingInitialize {
        PendingInitialize(self.outcomes.subscribe())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<InitializeRequest>> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A client `initialize` waiting for a relaunched child to answer it.
pub struct PendingInitialize(watch::Receiver<Option<Arc<Outcome>>>);

impl PendingInitialize {
    /// The answer the next child initialized with `request` gives.
    pub async fn answer(mut self, request: &InitializeRequest) -> Result<InitializeResponse> {
        let wait = async {
            loop {
                if self.0.changed().await.is_err() {
                    return Err(Error::internal_error().data("the acp-host stopped before the agent was relaunched"));
                }
                let outcome = self.0.borrow_and_update().clone();
                if let Some(outcome) = outcome.filter(|outcome| outcome.request == *request) {
                    return outcome.result.clone();
                }
            }
        };
        tokio::time::timeout(REINITIALIZE_TIMEOUT, wait)
            .await
            .unwrap_or_else(|_| {
                Err(Error::new(
                    ErrorCode::INTERNAL_ERROR,
                    "the agent was relaunched for this initialize but did not answer it in time",
                ))
            })
    }
}

#[cfg(test)]
mod tests;
