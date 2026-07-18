//! A native runner for [`ScenarioIr`] scenarios, executing directly against a
//! [`trogon_decider::Decider`] implementation instead of a compiled wasm component.
//!
//! Envelope decoding is kept identical to the wasm guest bridge by reusing the exact generic
//! functions the `export_decider!` macro expands into
//! ([`trogon_decider_guest_sdk::decode_command`], [`trogon_decider_guest_sdk::evolve_one`],
//! [`trogon_decider_guest_sdk::decide_command`]): the same code runs on both the wasm and the
//! native side, not a hand-mirrored copy of its logic.

use trogon_decider::{Decider, EventDecode, EventEncode, EventType};
use trogon_decider_guest_sdk::{
    AnyEnvelopeParts, AnyEnvelopeView, CommandEnvelopeView, DecideErrorView, DomainErrorParts, decide_command,
    decode_command, evolve_one,
};

use crate::ir::{ScenarioIr, StepOutcome, WireEnvelope};

impl CommandEnvelopeView for WireEnvelope {
    fn command_type(&self) -> &str {
        &self.type_url
    }

    fn command_payload(&self) -> &[u8] {
        &self.payload
    }
}

impl AnyEnvelopeView for WireEnvelope {
    fn event_type(&self) -> &str {
        &self.type_url
    }

    fn event_payload(&self) -> &[u8] {
        &self.payload
    }
}

impl From<AnyEnvelopeParts> for WireEnvelope {
    fn from(value: AnyEnvelopeParts) -> Self {
        Self {
            type_url: value.type_url,
            payload: value.payload,
        }
    }
}

/// Wire projection of a native decider's domain error.
///
/// Mirrors the WIT `domain-error` record's `code`/`message`/`details`, so a native rejection or
/// fault carries the same shape as the wasm guest bridge's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct NativeDomainError {
    /// The error's stable, machine-readable code.
    pub code: String,
    /// The error's human-readable message.
    pub message: String,
    /// The error's `#[source]` chain, flattened to ordered `(label, text)` pairs.
    pub details: Vec<(String, String)>,
}

impl From<DomainErrorParts> for NativeDomainError {
    fn from(value: DomainErrorParts) -> Self {
        Self {
            code: value.code,
            message: value.message,
            details: value.details,
        }
    }
}

/// A native `decide` call's failure, mirroring the WIT `decide-error` variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeDecideError {
    /// The command was rejected by a business rule.
    Rejected(NativeDomainError),
    /// The command failed for a reason other than a business rule.
    Faulted(NativeDomainError),
}

impl DecideErrorView for NativeDecideError {
    fn rejected(parts: DomainErrorParts) -> Self {
        Self::Rejected(parts.into())
    }

    fn faulted(parts: DomainErrorParts) -> Self {
        Self::Faulted(parts.into())
    }
}

/// Decodes a wire command envelope into a typed command.
///
/// Reuses the exact contract the wasm guest bridge's `decode_command` enforces: the envelope's
/// type URL must match `expected_type_url`, its payload decodes via `Proto::decode_from_slice`,
/// then converts to `Cmd` via `Cmd::try_from(proto)`.
pub fn decode_native_command<Proto, Cmd>(
    expected_type_url: &str,
    envelope: &WireEnvelope,
) -> Result<Cmd, NativeDomainError>
where
    Proto: buffa::Message,
    Cmd: TryFrom<Proto, Error: std::error::Error + 'static>,
{
    decode_command::<WireEnvelope, Proto, Cmd, NativeDomainError>(expected_type_url, envelope.clone())
}

/// Folds one wire event envelope into a decider's state.
///
/// Reuses the exact contract the wasm guest bridge's `evolve_one` enforces: an envelope outside
/// `C::Event`'s decoded set is skipped rather than treated as an error, mirroring native runtime
/// replay.
pub fn native_evolve_one<C>(state: C::State, envelope: &WireEnvelope) -> Result<C::State, NativeDomainError>
where
    C: Decider<EvolveError: std::error::Error + 'static>,
    C::Event: EventDecode,
{
    evolve_one::<C, NativeDomainError, WireEnvelope>(state, envelope.clone())
}

/// Decides a typed command against a decider's state.
///
/// Reuses the exact contract the wasm guest bridge's `decide_command` enforces: `decide` and
/// `evolve` run through the same `evaluate_decision` pipeline the native runtime uses, and each
/// resulting event is encoded through `EventEncode`/`EventType`.
pub fn native_decide<C>(command: &C, state: &C::State) -> Result<Vec<WireEnvelope>, NativeDecideError>
where
    C: Decider<DecideError: std::error::Error + 'static, EvolveError: std::error::Error + 'static, State: Clone>,
    C::Event: EventEncode + EventType,
{
    decide_command::<C, NativeDecideError, WireEnvelope>(command, state)
}

/// A native decider dispatch table, mirroring one `export_decider!` bundle: several commands
/// sharing one `State`, dispatched on a wire command envelope's type URL exactly the way the
/// wasm guest's generated `decide`/`evolve`/`stream_id` exports do.
pub trait NativeDeciderBundle {
    /// The bundle's shared decider state.
    type State: Clone;

    /// Returns the state before any events have been replayed.
    fn initial_state() -> Self::State;

    /// Resolves the stream id a command envelope targets.
    fn stream_id(command: &WireEnvelope) -> Result<String, NativeDomainError>;

    /// Folds one wire event envelope into the bundle's state.
    fn evolve(state: Self::State, event: &WireEnvelope) -> Result<Self::State, NativeDomainError>;

    /// Decides a wire command envelope against the bundle's state.
    fn decide(command: &WireEnvelope, state: &Self::State) -> Result<Vec<WireEnvelope>, NativeDecideError>;
}

/// Failure replaying a [`ScenarioIr`]'s `given` history or a step's forwarded events against a
/// [`NativeDeciderBundle`], before that step's outcome could be captured.
#[derive(Debug, thiserror::Error)]
pub enum NativeRunError {
    /// Replaying the scenario's `given` history failed at the event with this index.
    #[error("failed to replay given event {index}: {source}")]
    Given {
        /// The zero-based index of the failing `given` event.
        index: usize,
        /// The domain error the failing evolve produced.
        #[source]
        source: NativeDomainError,
    },
    /// Folding step `index`'s emitted events into state before the next step failed.
    #[error("failed to fold step {index}'s emitted events: {source}")]
    Fold {
        /// The zero-based index of the step whose emitted events failed to fold.
        index: usize,
        /// The domain error the failing evolve produced.
        #[source]
        source: NativeDomainError,
    },
}

/// Runs a [`ScenarioIr`] against a [`NativeDeciderBundle`], capturing each step's raw
/// [`StepOutcome`].
///
/// Mirrors [`ScenarioIr::run_wasm`]'s session shape: `given` is replayed once, then each step's
/// command is decided against the accumulated state and its emitted events (if any) are folded
/// in before the next step, all without asserting the step's declared expectation.
pub fn run_native<N: NativeDeciderBundle>(scenario: &ScenarioIr) -> Result<Vec<StepOutcome>, NativeRunError> {
    let mut state = N::initial_state();
    for (index, event) in scenario.given.iter().enumerate() {
        state = N::evolve(state, event).map_err(|source| NativeRunError::Given { index, source })?;
    }

    let mut outcomes = Vec::with_capacity(scenario.steps.len());
    for (index, step) in scenario.steps.iter().enumerate() {
        match N::decide(&step.when, &state) {
            Ok(events) => {
                for event in &events {
                    state = N::evolve(state, event).map_err(|source| NativeRunError::Fold { index, source })?;
                }
                outcomes.push(StepOutcome::Events(events));
            }
            Err(NativeDecideError::Rejected(error)) => outcomes.push(StepOutcome::Rejected {
                code: error.code,
                message: error.message,
            }),
            Err(NativeDecideError::Faulted(error)) => outcomes.push(StepOutcome::Faulted {
                code: error.code,
                message: error.message,
            }),
        }
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests;
