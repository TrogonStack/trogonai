//! Decider-agnostic scenario representation shared by every runner.
//!
//! A [`ScenarioIr`] captures a given/when/then scenario purely in wire form: type URLs and
//! encoded payload bytes, with no dependency on any specific decider's Rust types. Both the
//! wasm runner ([`ScenarioIr::to_sim_scenario`], driving a compiled component through
//! [`SimScenario`], and [`ScenarioIr::run_wasm`], capturing a component's raw outcomes) and a
//! native runner (see the `native` module, gated behind the `test-support` feature) consume the
//! same `ScenarioIr` value, so one scenario can be executed through either path without being
//! re-specified per path.

use trogon_decider_wit::host;

use crate::host::SimInstance;
use crate::scenario::{ScenarioError, SimScenario};

/// A wire-form event or command envelope: a type URL plus its encoded payload.
///
/// Commands are tagged with the full `type.googleapis.com/`-prefixed URL; events are tagged
/// with the bare protobuf message full name. Both runners interpret a `WireEnvelope`'s
/// `type_url` the same way real wire traffic would, so a codec divergence between the wasm
/// guest and a native decider shows up as a type or payload mismatch instead of being silently
/// normalized away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireEnvelope {
    /// The envelope's type URL: full for commands, bare protobuf full name for events.
    pub type_url: String,
    /// The encoded payload bytes.
    pub payload: Vec<u8>,
}

impl WireEnvelope {
    /// Creates a wire envelope from a type URL and payload bytes.
    pub fn new(type_url: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            type_url: type_url.into(),
            payload: payload.into(),
        }
    }
}

impl From<&WireEnvelope> for host::AnyEnvelope {
    fn from(value: &WireEnvelope) -> Self {
        Self {
            type_: value.type_url.clone(),
            payload: value.payload.clone(),
        }
    }
}

impl From<&WireEnvelope> for host::CommandEnvelope {
    fn from(value: &WireEnvelope) -> Self {
        Self {
            type_: value.type_url.clone(),
            payload: value.payload.clone(),
        }
    }
}

impl From<host::AnyEnvelope> for WireEnvelope {
    fn from(value: host::AnyEnvelope) -> Self {
        Self {
            type_url: value.type_,
            payload: value.payload,
        }
    }
}

impl From<host::CommandEnvelope> for WireEnvelope {
    fn from(value: host::CommandEnvelope) -> Self {
        Self {
            type_url: value.type_,
            payload: value.payload,
        }
    }
}

/// The declared outcome a [`ScenarioStep`] expects from its `when` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedOutcome {
    /// The command must be accepted and produce exactly these events, in order.
    Events(Vec<WireEnvelope>),
    /// The command must be rejected by a business rule (not a fault).
    Rejected,
    /// The command must be accepted, without asserting which events it produces.
    Accepted,
    /// The command must fail (rejected or faulted) with this code or message.
    Error(String),
}

/// One `when`/`then` pair in a scenario's ordered step sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioStep {
    /// The command envelope decided against the state accumulated so far.
    pub when: WireEnvelope,
    /// The outcome this step's `when` command is expected to produce.
    pub expect: ExpectedOutcome,
}

/// A domain error's shape, shared by every raw outcome that can carry a rejection or fault:
/// [`StepOutcome::Rejected`], [`StepOutcome::Faulted`], and [`StreamIdOutcome::Failed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainErrorOutcome {
    /// The domain error's stable, machine-readable code.
    pub code: String,
    /// The domain error's human-readable message.
    pub message: String,
    /// The domain error's causal chain, as ordered `(label, text)` pairs, most specific cause
    /// last.
    pub details: Vec<(String, String)>,
}

impl From<host::DomainError> for DomainErrorOutcome {
    fn from(value: host::DomainError) -> Self {
        Self {
            code: value.code,
            message: value.message,
            details: value.details,
        }
    }
}

/// The actual outcome one step produced, captured raw rather than checked against a declared
/// [`ExpectedOutcome`].
///
/// The parity harness compares a native runner's and a wasm runner's `StepOutcome` values
/// directly against each other, independent of what the scenario declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// The command was accepted, producing these events in order.
    Events(Vec<WireEnvelope>),
    /// The command was rejected by a business rule.
    Rejected(DomainErrorOutcome),
    /// The command failed for a reason other than a business rule: an unknown command type, a
    /// decode failure, an encode failure, and so on.
    Faulted(DomainErrorOutcome),
}

/// The stream id a scenario's first command resolved to, or the failure resolving it produced.
///
/// Captured raw the same way [`StepOutcome`] captures a step's decide outcome, so the parity
/// harness can compare a native runner's and a wasm runner's resolved stream id directly against
/// each other, independent of what the scenario declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamIdOutcome {
    /// The command's stream id resolved successfully.
    Resolved(String),
    /// Resolving the command's stream id failed.
    Failed(DomainErrorOutcome),
}

/// Everything one runner produced from a full run of a [`ScenarioIr`]: the stream id its first
/// command resolved to (`None` if the scenario has no steps), and each step's raw
/// [`StepOutcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioRun {
    /// The scenario's first command's resolved stream id, or `None` if the scenario has no
    /// steps.
    pub stream_id: Option<StreamIdOutcome>,
    /// Each step's raw outcome, in order.
    pub steps: Vec<StepOutcome>,
}

/// A decider-agnostic given/when/then scenario, expressed entirely in wire form.
///
/// `ScenarioIr` is the shared source of truth both runners execute: the CLI parses its YAML
/// `Scenario`/`Step` shapes into this IR, the wasm runner builds a [`SimScenario`] from it via
/// [`ScenarioIr::to_sim_scenario`] (or captures raw outcomes via [`ScenarioIr::run_wasm`]), and a
/// native runner (see the `native` module) executes it directly against a
/// [`trogon_decider::Decider`](https://docs.rs/trogon-decider) implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioIr {
    /// The scenario's human-readable name, used in test and CLI output.
    pub name: String,
    /// The stream id every command in this scenario targets, when known ahead of time.
    ///
    /// `assert_parity` resolves the first step's command's stream id independently through both
    /// runners (via `Decider::stream_id` natively, or the WIT `stream-id` export in wasm) and
    /// fails on any divergence between the two. When this field is `Some`, it also fails if the
    /// resolved id does not match it.
    pub stream_id: Option<String>,
    /// The session's seeded history, replayed via `evolve` before the first step's command is
    /// decided.
    pub given: Vec<WireEnvelope>,
    /// The scenario's ordered `when`/`then` steps, run against a single open session.
    pub steps: Vec<ScenarioStep>,
}

impl ScenarioIr {
    /// Creates an empty scenario with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            stream_id: None,
            given: Vec::new(),
            steps: Vec::new(),
        }
    }

    /// Returns the command envelope of this scenario's first step, if any.
    pub fn first_command(&self) -> Option<&WireEnvelope> {
        self.steps.first().map(|step| &step.when)
    }

    /// Builds a [`SimScenario`] that asserts each step's declared [`ExpectedOutcome`], ready to
    /// run against a wasm component instance via [`SimScenario::run`].
    pub fn to_sim_scenario(&self) -> SimScenario {
        let mut sim = SimScenario::new().given(self.given.iter().map(host::AnyEnvelope::from));
        for step in &self.steps {
            sim = sim.when(host::CommandEnvelope::from(&step.when));
            sim = match &step.expect {
                ExpectedOutcome::Events(events) => sim.then_events(events.iter().map(host::AnyEnvelope::from)),
                ExpectedOutcome::Rejected => sim.then_rejected(),
                ExpectedOutcome::Accepted => sim.then_accepted(),
                ExpectedOutcome::Error(expected) => sim.then_error(expected.clone()),
            };
        }
        sim
    }

    /// Runs this scenario against a wasm component instance, capturing the first step's resolved
    /// stream id and each step's raw [`StepOutcome`] instead of asserting it against the step's
    /// declared [`ExpectedOutcome`].
    ///
    /// Used by the parity harness to compare the wasm runner's actual behavior against a native
    /// runner's.
    pub fn run_wasm<T>(&self, instance: &mut SimInstance<T>) -> Result<ScenarioRun, ScenarioError> {
        let stream_id = match self.first_command() {
            Some(command) => {
                let command = host::CommandEnvelope::from(command);
                let resolved = instance
                    .stream_id(&command)
                    .map_err(|source| ScenarioError::StreamIdCall { source })?;
                Some(match resolved {
                    Ok(id) => StreamIdOutcome::Resolved(id),
                    Err(err) => StreamIdOutcome::Failed(err.into()),
                })
            }
            None => None,
        };

        let mut session = instance
            .open_session(None)
            .map_err(|source| ScenarioError::OpenSession { source })?;

        if !self.given.is_empty() {
            let given: Vec<host::AnyEnvelope> = self.given.iter().map(host::AnyEnvelope::from).collect();
            session
                .evolve(&given)
                .map_err(|source| ScenarioError::EvolveCall { source })?
                .map_err(|err| ScenarioError::Evolve {
                    code: err.code,
                    message: err.message,
                })?;
        }

        let mut outcomes = Vec::with_capacity(self.steps.len());
        let mut forwarded: Vec<host::AnyEnvelope> = Vec::new();

        for step in &self.steps {
            if !forwarded.is_empty() {
                session
                    .evolve(&forwarded)
                    .map_err(|source| ScenarioError::EvolveCall { source })?
                    .map_err(|err| ScenarioError::Evolve {
                        code: err.code,
                        message: err.message,
                    })?;
                forwarded.clear();
            }

            let command = host::CommandEnvelope::from(&step.when);
            let outcome = session
                .decide(&command)
                .map_err(|source| ScenarioError::DecideCall { source })?;

            let step_outcome = match outcome {
                Ok(events) => {
                    let events: Vec<WireEnvelope> = events.into_iter().map(WireEnvelope::from).collect();
                    forwarded = events.iter().map(host::AnyEnvelope::from).collect();
                    StepOutcome::Events(events)
                }
                Err(host::DecideError::Rejected(err)) => StepOutcome::Rejected(err.into()),
                Err(host::DecideError::Faulted(err)) => StepOutcome::Faulted(err.into()),
            };
            outcomes.push(step_outcome);
        }

        Ok(ScenarioRun {
            stream_id,
            steps: outcomes,
        })
    }
}

#[cfg(test)]
mod tests;
