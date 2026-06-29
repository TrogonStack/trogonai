use trogon_decider_wit::host;

use crate::host::SimInstance;

/// Typed failure from running a [`SimScenario`]: a Wasmtime/session fault, an evolve error, or a
/// then-expectation mismatch. Each variant keeps its structured context (the source Wasmtime error
/// or the domain error's code/message) instead of flattening it into a string.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("scenario missing .when(...)")]
    MissingWhen,
    #[error("scenario missing .then_events(...) or .then_rejected()")]
    MissingExpectation,
    #[error("failed to open session")]
    OpenSession {
        #[source]
        source: wasmtime::Error,
    },
    #[error("failed to call evolve")]
    EvolveCall {
        #[source]
        source: wasmtime::Error,
    },
    #[error("failed to call decide")]
    DecideCall {
        #[source]
        source: wasmtime::Error,
    },
    #[error("evolve failed: {code} — {message}")]
    Evolve { code: String, message: String },
    #[error("expected error '{expected}', got rejection: {code} — {message}")]
    ErrorGotRejection {
        expected: String,
        code: String,
        message: String,
    },
    #[error("expected error '{expected}', got fault: {code} — {message}")]
    ErrorGotFault {
        expected: String,
        code: String,
        message: String,
    },
    #[error("expected error '{expected}', got {count} event(s)")]
    ErrorGotEvents { expected: String, count: usize },
    #[error("expected rejection, got {count} event(s)")]
    RejectionGotEvents { count: usize },
    #[error("expected rejection, got fault: {code} — {message}")]
    RejectionGotFault { code: String, message: String },
    #[error("expected acceptance, got rejection: {code} — {message}")]
    AcceptanceGotRejection { code: String, message: String },
    #[error("expected acceptance, got fault: {code} — {message}")]
    AcceptanceGotFault { code: String, message: String },
    #[error("rejected: {code} — {message}")]
    EventsGotRejection { code: String, message: String },
    #[error("faulted: {code} — {message}")]
    EventsGotFault { code: String, message: String },
    #[error("expected {expected} event(s), got {actual}")]
    EventCountMismatch { expected: usize, actual: usize },
    #[error(
        "event {index} mismatch: got type={got_type} payload={got_payload:?}, want type={want_type} payload={want_payload:?}"
    )]
    EventMismatch {
        index: usize,
        got_type: String,
        got_payload: Vec<u8>,
        want_type: String,
        want_payload: Vec<u8>,
    },
}

/// Fluent given/when/then helper over a loaded WASM decider component.
#[must_use = "sim scenarios must be completed with .run()"]
pub struct SimScenario {
    given: Vec<host::AnyEnvelope>,
    when: Option<host::CommandEnvelope>,
    expect_events: Option<Vec<host::AnyEnvelope>>,
    expect_rejected: bool,
    expect_accepted: bool,
    expect_error: Option<String>,
}

impl SimScenario {
    pub fn new() -> Self {
        Self {
            given: Vec::new(),
            when: None,
            expect_events: None,
            expect_rejected: false,
            expect_accepted: false,
            expect_error: None,
        }
    }

    pub fn given(mut self, events: impl IntoIterator<Item = host::AnyEnvelope>) -> Self {
        self.given.extend(events);
        self
    }

    pub fn when(mut self, command: host::CommandEnvelope) -> Self {
        self.when = Some(command);
        self
    }

    pub fn then_events(mut self, events: impl IntoIterator<Item = host::AnyEnvelope>) -> Self {
        self.expect_events = Some(events.into_iter().collect());
        self.expect_rejected = false;
        self.expect_accepted = false;
        self.expect_error = None;
        self
    }

    pub fn then_rejected(mut self) -> Self {
        self.expect_events = None;
        self.expect_rejected = true;
        self.expect_accepted = false;
        self.expect_error = None;
        self
    }

    /// Expect the command to be accepted (decide returns events), without
    /// asserting which events. Used for scenarios that only assert "not rejected".
    pub fn then_accepted(mut self) -> Self {
        self.expect_events = None;
        self.expect_rejected = false;
        self.expect_accepted = true;
        self.expect_error = None;
        self
    }

    /// Expect the command to error (rejected or faulted) with the given code or
    /// message. Matches the decode/decide outcome's `code` or `message` exactly.
    pub fn then_error(mut self, expected: impl Into<String>) -> Self {
        self.expect_events = None;
        self.expect_rejected = false;
        self.expect_accepted = false;
        self.expect_error = Some(expected.into());
        self
    }

    pub fn run<T>(self, instance: &mut SimInstance<T>) -> Result<(), ScenarioError> {
        let command = self.when.ok_or(ScenarioError::MissingWhen)?;

        let mut session = instance
            .open_session(None)
            .map_err(|source| ScenarioError::OpenSession { source })?;

        if !self.given.is_empty() {
            session
                .evolve(&self.given)
                .map_err(|source| ScenarioError::EvolveCall { source })?
                .map_err(|err| ScenarioError::Evolve {
                    code: err.code,
                    message: err.message,
                })?;
        }

        let outcome = session
            .decide(&command)
            .map_err(|source| ScenarioError::DecideCall { source })?;

        if let Some(expected) = self.expect_error {
            let matches = |err: &host::DomainError| err.code == expected || err.message == expected;
            match outcome {
                Err(host::DecideError::Rejected(err)) | Err(host::DecideError::Faulted(err)) if matches(&err) => Ok(()),
                Err(host::DecideError::Rejected(err)) => Err(ScenarioError::ErrorGotRejection {
                    expected,
                    code: err.code,
                    message: err.message,
                }),
                Err(host::DecideError::Faulted(err)) => Err(ScenarioError::ErrorGotFault {
                    expected,
                    code: err.code,
                    message: err.message,
                }),
                Ok(events) => Err(ScenarioError::ErrorGotEvents {
                    expected,
                    count: events.len(),
                }),
            }
        } else if self.expect_rejected {
            match outcome {
                Err(host::DecideError::Rejected(_)) => Ok(()),
                Ok(events) => Err(ScenarioError::RejectionGotEvents { count: events.len() }),
                Err(host::DecideError::Faulted(err)) => Err(ScenarioError::RejectionGotFault {
                    code: err.code,
                    message: err.message,
                }),
            }
        } else if self.expect_accepted {
            match outcome {
                Ok(_) => Ok(()),
                Err(host::DecideError::Rejected(err)) => Err(ScenarioError::AcceptanceGotRejection {
                    code: err.code,
                    message: err.message,
                }),
                Err(host::DecideError::Faulted(err)) => Err(ScenarioError::AcceptanceGotFault {
                    code: err.code,
                    message: err.message,
                }),
            }
        } else {
            let expected = self.expect_events.ok_or(ScenarioError::MissingExpectation)?;
            let actual = outcome.map_err(|err| match err {
                host::DecideError::Rejected(err) => ScenarioError::EventsGotRejection {
                    code: err.code,
                    message: err.message,
                },
                host::DecideError::Faulted(err) => ScenarioError::EventsGotFault {
                    code: err.code,
                    message: err.message,
                },
            })?;
            if actual.len() != expected.len() {
                return Err(ScenarioError::EventCountMismatch {
                    expected: expected.len(),
                    actual: actual.len(),
                });
            }
            for (index, (got, want)) in actual.iter().zip(expected.iter()).enumerate() {
                if !events_match(got, want) {
                    return Err(ScenarioError::EventMismatch {
                        index,
                        got_type: got.type_.clone(),
                        got_payload: got.payload.clone(),
                        want_type: want.type_.clone(),
                        want_payload: want.payload.clone(),
                    });
                }
            }
            Ok(())
        }
    }
}

impl Default for SimScenario {
    fn default() -> Self {
        Self::new()
    }
}

/// Compares two event envelopes by meaning rather than by raw wire bytes.
///
/// Protobuf encoding is not canonical, so semantically identical events can
/// differ on the wire. When both payloads decode to known event types, compare
/// their canonical JSON; otherwise fall back to an exact byte match.
fn events_match(got: &host::AnyEnvelope, want: &host::AnyEnvelope) -> bool {
    if got.type_ != want.type_ {
        return false;
    }
    match (
        trogonai_proto::decode_event_to_json(&got.type_, &got.payload),
        trogonai_proto::decode_event_to_json(&want.type_, &want.payload),
    ) {
        (Some(got_json), Some(want_json)) => got_json == want_json,
        _ => got.payload == want.payload,
    }
}

#[cfg(test)]
mod tests;
