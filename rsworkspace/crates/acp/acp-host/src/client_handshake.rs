use crate::constants::{INITIALIZE_TIMEOUT, REINITIALIZE_TIMEOUT, RELAUNCH_COALESCE, RELAUNCH_COALESCE_LIMIT};
use agent_client_protocol::v1::schema::{InitializeRequest, InitializeResponse};
use agent_client_protocol::{Error, ErrorCode, Result, is_connection_terminated, is_incoming_transport_closed};
use serde::Deserialize;
use std::sync::{Mutex, PoisonError};
use tokio::sync::{Notify, oneshot};

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

/// One `initialize` a child answered.
#[derive(Debug, Clone)]
pub struct Handshake {
    pub request: ClientInitialize,
    pub response: InitializeResponse,
}

/// The claiming client's `initialize`, kept across child runs so that every
/// child the host launches is initialized the way that client asked, with
/// the params it sent (see [`ClientInitialize`]).
///
/// A run whose child is launched while a request is remembered initializes it
/// with that request before serving: a relaunch after a crash, or the relaunch
/// a client asks for by initializing again with different parameters. That
/// run reports how the child answered through [`Self::settle`], which answers
/// every client `initialize` waiting on those params.
///
/// Relaunches the client asks for are serialized here. At most one request is
/// pending (the one the next child gets), and asking for another one
/// supersedes it: every `initialize` still waiting on the older params fails
/// at once with an error saying so, and the next child is launched once, with
/// the newest params. A burst of different `initialize`s therefore costs one
/// relaunch, not one each, and none of them hangs.
#[derive(Debug, Default)]
pub struct ClientHandshake {
    state: Mutex<State>,
    /// Woken whenever a client asks for a relaunch.
    relaunch: Notify,
}

#[derive(Debug, Default)]
struct State {
    /// The `initialize` the next child is to be launched with.
    next: Option<ClientInitialize>,
    /// The client's `initialize` the latest child answered, and its answer.
    applied: Option<Handshake>,
    /// Client `initialize`s waiting for a child launched with their params.
    waiters: Vec<Waiter>,
}

#[derive(Debug)]
struct Waiter {
    request: ClientInitialize,
    answer: oneshot::Sender<Result<InitializeResponse>>,
}

impl ClientHandshake {
    /// The `initialize` the next child is to be launched with, if a client has
    /// sent one the host still holds.
    pub fn next(&self) -> Option<ClientInitialize> {
        self.lock().next.clone()
    }

    /// Records the client's first `initialize` of a child that no request was
    /// remembered for, as that child answered it, and keeps it for every later
    /// child.
    pub fn initialized(&self, done: &Handshake) {
        let mut state = self.lock();
        state.next = Some(done.request.clone());
        state.applied = Some(done.clone());
    }

    /// Asks for a child launched with `request`, which differs from what the
    /// running child answered, and returns the wait for that child's answer.
    ///
    /// A request a newer child already answered gets that answer at once. Any
    /// other request becomes the pending one, and the `initialize`s waiting on
    /// a different, older one are told they were superseded.
    pub fn relaunch_with(&self, request: ClientInitialize) -> PendingInitialize {
        let (answer, answered) = oneshot::channel();
        let mut state = self.lock();
        if let Some(done) = state.applied.as_ref().filter(|done| done.request == request) {
            let _ = answer.send(Ok(done.response.clone()));
            return PendingInitialize(answered);
        }
        for waiter in state.waiters.extract_if(.., |waiter| waiter.request != request) {
            let _ = waiter.answer.send(Err(superseded()));
        }
        state.waiters.push(Waiter {
            request: request.clone(),
            answer,
        });
        state.next = Some(request);
        drop(state);
        self.relaunch.notify_waiters();
        PendingInitialize(answered)
    }

    /// Resolves once a client has asked for params the latest child was not
    /// initialized with (including while it was being initialized), and then
    /// asked for nothing new for [`RELAUNCH_COALESCE`], or for at most
    /// [`RELAUNCH_COALESCE_LIMIT`]: the running child is to be replaced. It
    /// keeps serving until then, so the rest of a burst of `initialize`s
    /// reaches it and joins the one relaunch.
    pub async fn relaunch_requested(&self) {
        self.relaunch_wanted().await;
        let limit = tokio::time::sleep(RELAUNCH_COALESCE_LIMIT);
        tokio::pin!(limit);
        loop {
            tokio::select! {
                () = tokio::time::sleep(RELAUNCH_COALESCE) => return,
                () = &mut limit => return,
                () = self.relaunch.notified() => {}
            }
        }
    }

    pub async fn relaunch_wanted(&self) {
        loop {
            let notified = self.relaunch.notified();
            tokio::pin!(notified);
            // Registered before the check, so a request made right after it
            // still wakes this.
            notified.as_mut().enable();
            if self.wants_relaunch() {
                return;
            }
            notified.await;
        }
    }

    fn wants_relaunch(&self) -> bool {
        let state = self.lock();
        state
            .next
            .as_ref()
            .is_some_and(|next| state.applied.as_ref().is_none_or(|done| done.request != *next))
    }

    /// Records how a freshly launched child answered `request`, and answers
    /// every client `initialize` waiting on it. A request the child refused is
    /// dropped, so the next child is not launched into the same refusal: it
    /// waits for the client instead. A child that died before answering
    /// refused nothing, so the request is kept for the next one.
    pub fn settle(&self, request: &ClientInitialize, result: &Result<InitializeResponse>) {
        let mut state = self.lock();
        match result {
            Ok(response) => {
                state.applied = Some(Handshake {
                    request: request.clone(),
                    response: response.clone(),
                });
            }
            Err(error) if refused(error) && state.next.as_ref() == Some(request) => state.next = None,
            Err(_) => {}
        }
        answer_waiters(&mut state, request, result);
    }

    /// Records that a freshly launched child did not answer `request` within
    /// [`INITIALIZE_TIMEOUT`], and returns the error to fail its run with. A
    /// silent child refused nothing, so the request is kept for the next one.
    pub fn time_out(&self, request: &ClientInitialize) -> Error {
        let error = Error::new(
            ErrorCode::INTERNAL_ERROR,
            format!("the agent did not answer initialize within {INITIALIZE_TIMEOUT:?}"),
        );
        answer_waiters(&mut self.lock(), request, &Err(error.clone()));
        error
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn answer_waiters(state: &mut State, request: &ClientInitialize, result: &Result<InitializeResponse>) {
    for waiter in state.waiters.extract_if(.., |waiter| waiter.request == *request) {
        let _ = waiter.answer.send(result.clone());
    }
}

fn superseded() -> Error {
    Error::new(
        ErrorCode::INVALID_REQUEST,
        "the client sent another initialize with different parameters before the agent was relaunched with \
         this one; the agent is relaunched with the newer one",
    )
}

/// Whether the child answered with this error, as opposed to going away
/// before it answered.
fn refused(error: &Error) -> bool {
    !is_incoming_transport_closed(error) && !is_connection_terminated(error)
}

/// A client `initialize` waiting for a relaunched child to answer it.
pub struct PendingInitialize(oneshot::Receiver<Result<InitializeResponse>>);

impl PendingInitialize {
    /// The answer of the child launched with this request's params, or why
    /// there is none.
    pub async fn answer(self) -> Result<InitializeResponse> {
        match tokio::time::timeout(REINITIALIZE_TIMEOUT, self.0).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::internal_error().data("the acp-host stopped before the agent was relaunched")),
            Err(_) => Err(Error::new(
                ErrorCode::INTERNAL_ERROR,
                "the agent was relaunched for this initialize but did not answer it in time",
            )),
        }
    }
}

#[cfg(test)]
mod tests;
