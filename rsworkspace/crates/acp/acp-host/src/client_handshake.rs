use crate::constants::REINITIALIZE_TIMEOUT;
use agent_client_protocol::v1::schema::{InitializeRequest, InitializeResponse};
use agent_client_protocol::{Error, ErrorCode, Result, is_connection_terminated, is_incoming_transport_closed};
use serde::Deserialize;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

/// A client's `initialize` params exactly as sent, with the typed view the
/// host reads (`clientInfo`, for the claim).
///
/// The child gets the params, not the typed request: the v1
/// `ClientCapabilities` keeps only the members the schema names, and fx reads
/// the host tools a client offers from a member of its own,
/// `clientCapabilities.libtrogonai`. Two are the same `initialize` when their
/// params are equal JSON.
#[derive(Debug, Clone)]
pub struct ClientInitialize {
    params: serde_json::Value,
    typed: InitializeRequest,
}

impl ClientInitialize {
    /// Reads `params` as an `initialize`, refusing what is not one.
    pub fn from_params(params: serde_json::Value) -> Result<Self> {
        let typed = InitializeRequest::deserialize(&params).map_err(|e| Error::invalid_params().data(e.to_string()))?;
        Ok(Self { params, typed })
    }

    pub fn params(&self) -> &serde_json::Value {
        &self.params
    }

    pub fn typed(&self) -> &InitializeRequest {
        &self.typed
    }
}

impl From<InitializeRequest> for ClientInitialize {
    fn from(typed: InitializeRequest) -> Self {
        Self {
            params: serde_json::to_value(&typed).unwrap_or_default(),
            typed,
        }
    }
}

impl PartialEq for ClientInitialize {
    fn eq(&self, other: &Self) -> bool {
        self.params == other.params
    }
}

/// The claiming client's `initialize`, kept across child runs so that every
/// child the host launches is initialized the way that client asked, with
/// the params it sent (see [`ClientInitialize`]).
///
/// A run whose child is launched while a request is remembered initializes it
/// with that request before serving: a relaunch after a crash, or the relaunch
/// a client asks for by initializing again with different parameters. That
/// run reports how the child answered through [`Self::settle`], so the
/// client's `initialize` that asked for the relaunch can return the new
/// child's real response.
#[derive(Debug)]
pub struct ClientHandshake {
    last: Mutex<Option<ClientInitialize>>,
    outcomes: watch::Sender<Option<Arc<Outcome>>>,
}

#[derive(Debug)]
struct Outcome {
    request: ClientInitialize,
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
    pub fn last(&self) -> Option<ClientInitialize> {
        self.lock().clone()
    }

    /// Keeps `request` for every later child.
    pub fn remember(&self, request: ClientInitialize) {
        *self.lock() = Some(request);
    }

    /// Records how a freshly launched child answered `request`, for the
    /// client waiting on it. A request the child refused is dropped, so the
    /// next child is not launched into the same refusal: it waits for the
    /// client instead. A child that died before answering refused nothing, so
    /// the request is kept for the next one.
    pub fn settle(&self, request: &ClientInitialize, result: &Result<InitializeResponse>) {
        if result.as_ref().is_err_and(refused) {
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<ClientInitialize>> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether the child answered with this error, as opposed to going away
/// before it answered.
fn refused(error: &Error) -> bool {
    !is_incoming_transport_closed(error) && !is_connection_terminated(error)
}

/// A client `initialize` waiting for a relaunched child to answer it.
pub struct PendingInitialize(watch::Receiver<Option<Arc<Outcome>>>);

impl PendingInitialize {
    /// The answer the next child initialized with `request` gives.
    pub async fn answer(mut self, request: &ClientInitialize) -> Result<InitializeResponse> {
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
