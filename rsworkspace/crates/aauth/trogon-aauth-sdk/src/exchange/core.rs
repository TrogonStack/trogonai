//! Sans-io core of the Deferred Response State Machine, per draft "Deferred
//! Responses". No I/O: this module only knows about typed events ("what came
//! back") and actions ("what to send next"). The reqwest driver in
//! `exchange::client` is the only piece that touches the network; this file
//! is testable with hand-built events and no HTTP mocking.
//!
//! States: `AwaitingInitial` (before the first response), `Polling` (holds
//! the `Location` URL and current backoff interval), `Interacting` (holds
//! `Location` plus the surfaced interaction url/code), `AwaitingClarification`
//! (holds `Location` plus the surfaced question), and the terminal states
//! reached once a response resolves the exchange.
//!
//! Events model exactly the status codes the "Deferred Response State
//! Machine" diagram distinguishes on the initial POST (`200`, `202`, `400`,
//! `401`, `402`, `500`, `503`) and on the poll GET (`200`, `202`, `403`,
//! `408`, `410`, `429`, `500`, `503`), plus the clarification sub-actions
//! (response / updated-request / cancel) which are POST/DELETE against the
//! same `Location`, not part of the GET polling loop.

use trogon_identity_types::aauth::Requirement;
use trogon_identity_types::aauth::error::{ErrorResponse, PollingError};
use trogon_identity_types::aauth::person_server::{PendingResponse, TokenGrantResponse};

/// Headers carried on a `202` response that the JSON body does not include,
/// per "Deferred Responses" / "Pending Response" -- `Location` and
/// `Retry-After` are REQUIRED HTTP headers, not body fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingHeaders {
    pub location: String,
    pub retry_after_secs: u64,
    pub requirement: Option<Requirement>,
}

/// What came back from an HTTP round trip, translated into the sans-io
/// vocabulary the core state machine understands. The reqwest driver builds
/// these from real `reqwest::Response`s; tests build them by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeEvent {
    /// `200` on the initial POST or a poll GET: done, process the body.
    Granted(TokenGrantResponse),
    /// `202` on the initial POST or a poll GET: still pending.
    Pending {
        headers: PendingHeaders,
        body: PendingResponse,
    },
    /// `400` on the initial POST: invalid request, per "Token Endpoint Error Codes".
    InvalidRequest(ErrorResponse),
    /// `401` on the initial POST: invalid signature, or (per "Resource Access
    /// and Resource Tokens") a resource challenge requiring an auth token.
    Unauthorized(ErrorResponse),
    /// `402` on the initial POST: payment required alongside the same
    /// `AAuth-Requirement` header shape as a resource challenge.
    PaymentRequired(PendingHeaders),
    /// `403` on a poll GET: denied or abandoned, per "Polling Error Codes".
    Denied(PollingError),
    /// `408` on a poll GET: expired; the agent MAY start a fresh request.
    Expired,
    /// `410` on a poll GET: gone; MUST NOT retry.
    Gone,
    /// `429` on a poll GET: slow down; increase interval by 5s.
    SlowDown,
    /// `500`: server error; start over.
    ServerError,
    /// `503`: temporarily unavailable; back off per `Retry-After` and retry.
    ServiceUnavailable { retry_after_secs: u64 },
}

/// Sans-io state of an in-progress token exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeState {
    /// No response received yet; the driver is about to send (or has just
    /// sent) the initial POST.
    AwaitingInitial,
    /// Received a `202` with no interaction/clarification requirement (or one
    /// already handled); polling `location` on the given interval.
    Polling { location: String, interval_secs: u64 },
    /// Received a `202` with `requirement=interaction`; surfaced to the
    /// caller, still holds `location` so polling can resume once the caller
    /// tells the state machine to keep going.
    Interacting {
        location: String,
        interval_secs: u64,
        url: String,
        code: Option<String>,
    },
    /// Received a `202` with a clarification question in the body; surfaced
    /// to the caller, holds `location` for the eventual response/update.
    AwaitingClarification {
        location: String,
        interval_secs: u64,
        question: String,
        timeout: Option<i64>,
        options: Option<Vec<String>>,
    },
    /// Exchange concluded successfully.
    Granted(TokenGrantResponse),
    /// Exchange concluded with a terminal, non-retryable outcome.
    Terminal(TerminalReason),
}

/// Why the exchange stopped without a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalReason {
    InvalidRequest(ErrorResponse),
    Unauthorized(ErrorResponse),
    PaymentRequired { location: String },
    Denied(PollingError),
    Expired,
    Gone,
    ServerError,
}

/// An instruction for the driver: either wait and poll again, or stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeAction {
    /// Sleep `after_secs`, then GET `location` (with `Prefer: wait=N`).
    PollAfter { location: String, after_secs: u64 },
    /// The exchange reached a state the caller must act on or a terminal
    /// state; nothing more to send automatically.
    Stop,
}

/// Default poll interval per "Polling with GET": used when a `202` response
/// omits `Retry-After` (the spec calls this a MUST-have header on `202`s, but
/// core stays defensive for malformed servers).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
/// Linear backoff step applied on `429 Too Many Requests`, per "Polling with GET".
pub const SLOW_DOWN_STEP_SECS: u64 = 5;

/// Pure transition function implementing the "Deferred Response State
/// Machine" diagram. Returns the new state and the action the driver should
/// take next.
#[must_use]
pub fn step(state: ExchangeState, event: ExchangeEvent) -> (ExchangeState, ExchangeAction) {
    match event {
        ExchangeEvent::Granted(grant) => (ExchangeState::Granted(grant), ExchangeAction::Stop),
        ExchangeEvent::Pending { headers, body } => pending_transition(headers, body),
        ExchangeEvent::InvalidRequest(err) => (
            ExchangeState::Terminal(TerminalReason::InvalidRequest(err)),
            ExchangeAction::Stop,
        ),
        ExchangeEvent::Unauthorized(err) => (
            ExchangeState::Terminal(TerminalReason::Unauthorized(err)),
            ExchangeAction::Stop,
        ),
        ExchangeEvent::PaymentRequired(headers) => (
            ExchangeState::Terminal(TerminalReason::PaymentRequired {
                location: headers.location,
            }),
            ExchangeAction::Stop,
        ),
        ExchangeEvent::Denied(reason) => (
            ExchangeState::Terminal(TerminalReason::Denied(reason)),
            ExchangeAction::Stop,
        ),
        ExchangeEvent::Expired => (ExchangeState::Terminal(TerminalReason::Expired), ExchangeAction::Stop),
        ExchangeEvent::Gone => (ExchangeState::Terminal(TerminalReason::Gone), ExchangeAction::Stop),
        ExchangeEvent::SlowDown => slow_down_transition(state),
        ExchangeEvent::ServerError => (
            ExchangeState::Terminal(TerminalReason::ServerError),
            ExchangeAction::Stop,
        ),
        ExchangeEvent::ServiceUnavailable { retry_after_secs } => {
            service_unavailable_transition(state, retry_after_secs)
        }
    }
}

fn pending_transition(headers: PendingHeaders, body: PendingResponse) -> (ExchangeState, ExchangeAction) {
    let location = headers.location;
    let interval_secs = headers.retry_after_secs;

    if let Some(question) = body.clarification.clone() {
        let state = ExchangeState::AwaitingClarification {
            location,
            interval_secs,
            question,
            timeout: body.timeout,
            options: body.options.clone(),
        };
        return (state, ExchangeAction::Stop);
    }

    if let Some(Requirement::Interaction { url, code }) = headers.requirement {
        let state = ExchangeState::Interacting {
            location,
            interval_secs,
            url,
            code,
        };
        return (state, ExchangeAction::Stop);
    }

    // status=interacting per "Pending Response": user has arrived at the
    // interaction endpoint, so the agent should stop actively prompting but
    // keeps polling the same Location.
    let action = ExchangeAction::PollAfter {
        location: location.clone(),
        after_secs: interval_secs,
    };
    (
        ExchangeState::Polling {
            location,
            interval_secs,
        },
        action,
    )
}

fn slow_down_transition(state: ExchangeState) -> (ExchangeState, ExchangeAction) {
    let (location, interval_secs) = match &state {
        ExchangeState::Polling {
            location,
            interval_secs,
        }
        | ExchangeState::Interacting {
            location,
            interval_secs,
            ..
        }
        | ExchangeState::AwaitingClarification {
            location,
            interval_secs,
            ..
        } => (location.clone(), *interval_secs),
        other => return (other.clone(), ExchangeAction::Stop),
    };
    let new_interval = interval_secs + SLOW_DOWN_STEP_SECS;
    let new_state = match state {
        ExchangeState::Interacting { url, code, .. } => ExchangeState::Interacting {
            location: location.clone(),
            interval_secs: new_interval,
            url,
            code,
        },
        ExchangeState::AwaitingClarification {
            question,
            timeout,
            options,
            ..
        } => ExchangeState::AwaitingClarification {
            location: location.clone(),
            interval_secs: new_interval,
            question,
            timeout,
            options,
        },
        _ => ExchangeState::Polling {
            location: location.clone(),
            interval_secs: new_interval,
        },
    };
    let action = ExchangeAction::PollAfter {
        location,
        after_secs: new_interval,
    };
    (new_state, action)
}

fn service_unavailable_transition(state: ExchangeState, retry_after_secs: u64) -> (ExchangeState, ExchangeAction) {
    let location = match &state {
        ExchangeState::Polling { location, .. }
        | ExchangeState::Interacting { location, .. }
        | ExchangeState::AwaitingClarification { location, .. } => location.clone(),
        ExchangeState::AwaitingInitial => {
            // 503 on the initial POST: no Location yet, caller must resend
            // the initial request after backing off. Nothing for `step` to
            // poll, so surface Stop and let the driver decide to retry the
            // initial POST itself.
            return (ExchangeState::AwaitingInitial, ExchangeAction::Stop);
        }
        other => return (other.clone(), ExchangeAction::Stop),
    };
    let action = ExchangeAction::PollAfter {
        location: location.clone(),
        after_secs: retry_after_secs,
    };
    let new_state = match state {
        ExchangeState::Interacting { url, code, .. } => ExchangeState::Interacting {
            location,
            interval_secs: retry_after_secs,
            url,
            code,
        },
        ExchangeState::AwaitingClarification {
            question,
            timeout,
            options,
            ..
        } => ExchangeState::AwaitingClarification {
            location,
            interval_secs: retry_after_secs,
            question,
            timeout,
            options,
        },
        _ => ExchangeState::Polling {
            location,
            interval_secs: retry_after_secs,
        },
    };
    (new_state, action)
}

#[cfg(test)]
mod tests;
