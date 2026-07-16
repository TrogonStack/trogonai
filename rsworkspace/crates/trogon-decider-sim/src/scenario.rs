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
    #[error("evolve failed: {code}: {message}")]
    Evolve { code: String, message: String },
    #[error("expected error '{expected}', got rejection: {code}: {message}")]
    ErrorGotRejection {
        expected: String,
        code: String,
        message: String,
    },
    #[error("expected error '{expected}', got fault: {code}: {message}")]
    ErrorGotFault {
        expected: String,
        code: String,
        message: String,
    },
    #[error("expected error '{expected}', got {count} event(s)")]
    ErrorGotEvents { expected: String, count: usize },
    #[error("expected rejection, got {count} event(s)")]
    RejectionGotEvents { count: usize },
    #[error("expected rejection, got fault: {code}: {message}")]
    RejectionGotFault { code: String, message: String },
    #[error("expected acceptance, got rejection: {code}: {message}")]
    AcceptanceGotRejection { code: String, message: String },
    #[error("expected acceptance, got fault: {code}: {message}")]
    AcceptanceGotFault { code: String, message: String },
    #[error("rejected: {code}: {message}")]
    EventsGotRejection { code: String, message: String },
    #[error("faulted: {code}: {message}")]
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
    /// A step in a multi-step scenario failed. `index` is the zero-based
    /// position of the failing step in the scenario's ordered step sequence.
    ///
    /// Only used when a scenario has more than one step; a scenario with
    /// exactly one step surfaces that step's failure unwrapped, unchanged
    /// from a scenario built with a single `.when(...)`/`.then_*(...)` pair.
    #[error("step {index}: {source}")]
    Step {
        index: usize,
        #[source]
        source: Box<ScenarioError>,
    },
}

/// The asserted outcome of a step's `decide` call.
#[derive(Debug)]
enum Expectation {
    Events(Vec<host::AnyEnvelope>),
    Rejected,
    Accepted,
    Error(String),
}

/// One `when`/`then` pair in a scenario's ordered step sequence.
struct ScenarioStep {
    when: host::CommandEnvelope,
    expectation: Expectation,
}

/// The step currently being built by the fluent API, before it is known
/// whether another `.when(...)` will follow it or `.run(...)` will finalize it.
#[derive(Default)]
struct PendingStep {
    when: Option<host::CommandEnvelope>,
    expectation: Option<Expectation>,
}

impl PendingStep {
    /// Moves this pending step into `steps` once both halves are present,
    /// leaving an incomplete pending step (missing `when` or `expectation`)
    /// untouched so `run` can report it as [`ScenarioError::MissingWhen`] or
    /// [`ScenarioError::MissingExpectation`].
    fn flush_into(&mut self, steps: &mut Vec<ScenarioStep>) {
        if self.when.is_none() || self.expectation.is_none() {
            return;
        }
        if let (Some(when), Some(expectation)) = (self.when.take(), self.expectation.take()) {
            steps.push(ScenarioStep { when, expectation });
        }
    }
}

/// Fluent given/when/then helper over a loaded WASM decider component.
///
/// A scenario runs an ordered sequence of one or more `when`/`then` steps
/// against a single open guest session, the way a real caller issuing several
/// commands in a row would: each step's emitted events are folded into the
/// session via `evolve` before the next step's command is decided, instead of
/// each step starting from a fresh session.
#[must_use = "sim scenarios must be completed with .run()"]
pub struct SimScenario {
    given: Vec<host::AnyEnvelope>,
    steps: Vec<ScenarioStep>,
    current: PendingStep,
}

impl SimScenario {
    pub fn new() -> Self {
        Self {
            given: Vec::new(),
            steps: Vec::new(),
            current: PendingStep::default(),
        }
    }

    /// Seeds the session's history, replayed via `evolve` before the first
    /// step's command is decided.
    pub fn given(mut self, events: impl IntoIterator<Item = host::AnyEnvelope>) -> Self {
        self.given.extend(events);
        self
    }

    /// Sets the command for a step.
    ///
    /// Calling this a second time after a preceding step already has both a
    /// command and a `.then_*(...)` expectation completes that step and
    /// starts the next one, building an ordered multi-step scenario. A single
    /// `.when(...)` call followed by exactly one `.then_*(...)` call behaves
    /// exactly as it always has.
    pub fn when(mut self, command: host::CommandEnvelope) -> Self {
        self.current.flush_into(&mut self.steps);
        self.current.when = Some(command);
        self
    }

    pub fn then_events(mut self, events: impl IntoIterator<Item = host::AnyEnvelope>) -> Self {
        self.current.expectation = Some(Expectation::Events(events.into_iter().collect()));
        self
    }

    pub fn then_rejected(mut self) -> Self {
        self.current.expectation = Some(Expectation::Rejected);
        self
    }

    /// Expect the command to be accepted (decide returns events), without
    /// asserting which events. Used for scenarios that only assert "not rejected".
    pub fn then_accepted(mut self) -> Self {
        self.current.expectation = Some(Expectation::Accepted);
        self
    }

    /// Expect the command to error (rejected or faulted) with the given code or
    /// message. Matches the decode/decide outcome's `code` or `message` exactly.
    pub fn then_error(mut self, expected: impl Into<String>) -> Self {
        self.current.expectation = Some(Expectation::Error(expected.into()));
        self
    }

    pub fn run<T>(mut self, instance: &mut SimInstance<T>) -> Result<(), ScenarioError> {
        self.current.flush_into(&mut self.steps);
        if self.current.when.is_some() {
            return Err(ScenarioError::MissingExpectation);
        }
        if self.steps.is_empty() {
            return Err(ScenarioError::MissingWhen);
        }

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

        let wrap_per_step = self.steps.len() > 1;
        let mut forwarded = Vec::new();

        for (index, step) in self.steps.into_iter().enumerate() {
            if !forwarded.is_empty() {
                session
                    .evolve(&forwarded)
                    .map_err(|source| wrap_step(index, wrap_per_step, ScenarioError::EvolveCall { source }))?
                    .map_err(|err| {
                        wrap_step(
                            index,
                            wrap_per_step,
                            ScenarioError::Evolve {
                                code: err.code,
                                message: err.message,
                            },
                        )
                    })?;
                forwarded.clear();
            }

            let outcome = session
                .decide(&step.when)
                .map_err(|source| wrap_step(index, wrap_per_step, ScenarioError::DecideCall { source }))?;
            forwarded =
                check_outcome(outcome, step.expectation).map_err(|error| wrap_step(index, wrap_per_step, error))?;
        }

        Ok(())
    }
}

impl Default for SimScenario {
    fn default() -> Self {
        Self::new()
    }
}

fn wrap_step(index: usize, wrap: bool, error: ScenarioError) -> ScenarioError {
    if wrap {
        ScenarioError::Step {
            index,
            source: Box::new(error),
        }
    } else {
        error
    }
}

/// Checks one step's `decide` outcome against its expectation, returning the
/// events actually emitted so the caller can fold them into the next step.
fn check_outcome(
    outcome: Result<Vec<host::AnyEnvelope>, host::DecideError>,
    expectation: Expectation,
) -> Result<Vec<host::AnyEnvelope>, ScenarioError> {
    match expectation {
        Expectation::Error(expected) => {
            let matches = |err: &host::DomainError| err.code == expected || err.message == expected;
            match outcome {
                Err(host::DecideError::Rejected(err)) | Err(host::DecideError::Faulted(err)) if matches(&err) => {
                    Ok(Vec::new())
                }
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
        }
        Expectation::Rejected => match outcome {
            Err(host::DecideError::Rejected(_)) => Ok(Vec::new()),
            Ok(events) => Err(ScenarioError::RejectionGotEvents { count: events.len() }),
            Err(host::DecideError::Faulted(err)) => Err(ScenarioError::RejectionGotFault {
                code: err.code,
                message: err.message,
            }),
        },
        Expectation::Accepted => match outcome {
            Ok(events) => Ok(events),
            Err(host::DecideError::Rejected(err)) => Err(ScenarioError::AcceptanceGotRejection {
                code: err.code,
                message: err.message,
            }),
            Err(host::DecideError::Faulted(err)) => Err(ScenarioError::AcceptanceGotFault {
                code: err.code,
                message: err.message,
            }),
        },
        Expectation::Events(expected) => {
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
            Ok(actual)
        }
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
        (Ok(Some(got_json)), Ok(Some(want_json))) => got_json == want_json,
        // Both unregistered (same type was checked above) → fall back to raw bytes.
        (Ok(None), Ok(None)) => got.payload == want.payload,
        // A registered type that fails to decode is malformed output, not a match.
        _ => false,
    }
}

#[cfg(test)]
mod tests;
