//! YAML test suite shapes, and their conversion into [`trogon_decider_sim::ScenarioIr`].
//!
//! Parsing (this module) and execution ([`crate::codec`] plus `trogon_decider_sim`) are kept
//! separate from the coverage-gap bookkeeping that only the `decider-test` binary needs, so a
//! parity test elsewhere in the workspace can parse a real YAML suite and run every scenario
//! through both a native decider and a compiled wasm component using this exact code path,
//! rather than a second hand-written parser.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use trogon_decider_sim::{ExpectedOutcome, ScenarioIr, ScenarioStep};

use crate::codec;

/// A YAML decider conformance suite: a named group of scenarios plus the events the suite
/// author declares the decider under test can produce.
#[derive(Debug, Deserialize)]
pub struct Suite {
    /// The suite's human-readable name.
    pub suite: String,
    /// Type URLs (bare or `type.googleapis.com/`-prefixed) of every event the decider under
    /// test can produce, asserted by the suite author. Compared against every event actually
    /// referenced across all scenarios' `given` and `then.events` entries for the strict
    /// coverage check, since neither the WIT descriptor nor the proto type registry can
    /// distinguish "events this decider emits" from unrelated message and helper types.
    #[serde(default)]
    pub events: Vec<String>,
    /// The suite's scenarios, run independently against a fresh component instance.
    pub scenarios: Vec<Scenario>,
}

impl Suite {
    /// Parses a suite from its YAML text.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).context("parse suite YAML")
    }

    /// Converts every scenario in this suite into [`ScenarioIr`], in declared order.
    pub fn to_ir(&self) -> Result<Vec<ScenarioIr>> {
        self.scenarios.iter().map(Scenario::to_ir).collect()
    }
}

/// One scenario: an optional seeded history, plus one or more `when`/`then` steps.
#[derive(Debug, Deserialize)]
pub struct Scenario {
    /// The scenario's human-readable name.
    pub name: String,
    /// The seeded event history, replayed before the first step's command is decided.
    #[serde(default)]
    pub given: Vec<serde_json::Value>,
    /// The legacy single-step command, mutually exclusive with `steps`.
    #[serde(default)]
    pub when: Option<serde_json::Value>,
    /// The legacy single-step expectation, mutually exclusive with `steps`.
    #[serde(default)]
    pub then: Option<Then>,
    /// An ordered `when`/`then` sequence run against a single open session, each step's emitted
    /// events folded in before the next step's command is decided. Mutually exclusive with the
    /// single `when`/`then` shape.
    #[serde(default)]
    pub steps: Option<Vec<Step>>,
}

/// One `when`/`then` pair in a scenario's `steps` list.
#[derive(Debug, Deserialize)]
pub struct Step {
    /// The command decided against the state accumulated so far.
    pub when: serde_json::Value,
    /// The outcome this step's command is expected to produce.
    pub then: Then,
}

impl Scenario {
    /// Returns this scenario's ordered when/then steps, normalizing the legacy single
    /// `when`/`then` shape into a one-element step list.
    ///
    /// Exactly one of `steps` (nonempty) or `when`+`then` must be present; any other
    /// combination is a malformed scenario.
    pub fn steps(&self) -> Result<Vec<(&serde_json::Value, &Then)>> {
        match (self.steps.as_ref(), self.when.as_ref(), self.then.as_ref()) {
            (Some(steps), None, None) => {
                if steps.is_empty() {
                    bail!("scenario '{}' has an empty steps list", self.name);
                }
                Ok(steps.iter().map(|step| (&step.when, &step.then)).collect())
            }
            (None, Some(when), Some(then)) => Ok(vec![(when, then)]),
            _ => bail!(
                "scenario '{}' must have exactly one of: a nonempty steps list, or both when and then",
                self.name
            ),
        }
    }

    /// Converts this scenario into decider-agnostic [`ScenarioIr`], decoding every `given`,
    /// `when`, and `then.events` value into wire form via [`crate::codec`].
    pub fn to_ir(&self) -> Result<ScenarioIr> {
        let mut ir = ScenarioIr::new(self.name.clone());
        for value in &self.given {
            ir.given.push(codec::json_any_to_envelope(value)?);
        }
        for (when, then) in self.steps()? {
            let when = codec::json_any_to_command(when)?;
            let expect = match then {
                Then::Events { events } => {
                    let events = events
                        .iter()
                        .map(codec::json_any_to_envelope)
                        .collect::<Result<Vec<_>>>()?;
                    ExpectedOutcome::Events(events)
                }
                Then::Rejected { rejected: true } => ExpectedOutcome::Rejected,
                Then::Rejected { rejected: false } => ExpectedOutcome::Accepted,
                Then::Error { error } => ExpectedOutcome::Error(error.expected()?),
            };
            ir.steps.push(ScenarioStep { when, expect });
        }
        Ok(ir)
    }
}

/// A step's declared expectation.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
pub enum Then {
    /// The command must be accepted and produce exactly these events, in order.
    Events {
        /// The expected events, in order.
        events: Vec<serde_json::Value>,
    },
    /// The command must fail with this code or message.
    Error {
        /// The expected error.
        error: ErrorExpectation,
    },
    /// The command must be accepted (`true`) or rejected (`false`), without asserting events.
    Rejected {
        /// Whether the command must be rejected.
        rejected: bool,
    },
}

/// `then.error` accepts either a bare string or the documented `{ code, message }` object; both
/// are matched against the domain error's code or message.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ErrorExpectation {
    /// The documented `{ code, message }` shape.
    Structured {
        /// The expected domain error code.
        #[serde(default)]
        code: Option<String>,
        /// The expected domain error message.
        #[serde(default)]
        message: Option<String>,
    },
    /// A bare string, matched against either the domain error's code or message.
    Plain(String),
}

impl ErrorExpectation {
    /// Returns the string this expectation is matched against.
    pub fn expected(&self) -> Result<String> {
        match self {
            Self::Plain(value) => Ok(value.clone()),
            Self::Structured { code, message } => code
                .clone()
                .or_else(|| message.clone())
                .context("then.error requires a code or message"),
        }
    }
}

#[cfg(test)]
mod tests;
