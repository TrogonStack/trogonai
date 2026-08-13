//! YAML test suite shapes, and their conversion into [`trogon_decider_sim::ScenarioIr`].
//!
//! Parsing (this module) and execution ([`crate::codec`] plus `trogon_decider_sim`) are kept
//! separate from the coverage-gap bookkeeping that only the `decider-test` binary needs, so a
//! parity test elsewhere in the workspace can parse a real YAML suite and run every scenario
//! through both a native decider and a compiled wasm component using this exact code path,
//! rather than a second hand-written parser.

use anyhow::{Context, Result, bail};
use buffa::type_registry::TypeRegistry;
use serde::Deserialize;
use trogon_decider_sim::{ExpectedOutcome, ScenarioIr, ScenarioStep};

use crate::codec;

/// A YAML decider conformance suite: a named group of scenarios plus the events the suite
/// author declares the decider under test can produce.
#[derive(Debug, Deserialize)]
pub struct Suite {
    /// The suite's human-readable name. Doubles as the decider's registered module identity: the
    /// same name its compiled component reports from `descriptor()`, used to resolve which type
    /// registry and which declared event set this suite's scenarios decode against.
    pub suite: String,
    /// Type URLs (bare or `type.googleapis.com/`-prefixed) of every event the decider under test
    /// can produce, as asserted by the suite author. Informational only: the strict coverage
    /// check now grounds its declared event set in the decider's own registration (see
    /// [`crate::codec::declared_events`]) instead of trusting this field, since a self-declared
    /// list can drift from what the component actually emits without anything catching it.
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

    /// Converts every scenario in this suite into [`ScenarioIr`], in declared order, decoding
    /// wire payloads against the type registry registered for this suite's `suite` module name.
    pub fn to_ir(&self) -> Result<Vec<ScenarioIr>> {
        let registry = codec::type_registry(&self.suite)?;
        self.scenarios.iter().map(|scenario| scenario.to_ir(registry)).collect()
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
    /// Fault-injection overrides this scenario runs the wasm component under, used together with
    /// `then.trap: true` to exercise a resource trap (fuel exhaustion, an expired epoch deadline,
    /// or a memory ceiling) end to end instead of the component's default production budget.
    #[serde(default)]
    pub budget: Option<BudgetOverrides>,
}

/// YAML-authored overrides to the wasm engine's fuel/epoch/memory budget for one scenario.
///
/// A `None` field leaves the component's default production budget for that resource untouched,
/// so a scenario only needs to name the budget it means to starve.
#[derive(Debug, Deserialize)]
pub struct BudgetOverrides {
    /// Overrides the fuel budget applied before each guest export call.
    #[serde(default)]
    pub fuel_per_call: Option<u64>,
    /// Overrides the epoch ticks allowed for a single guest export call before it is interrupted.
    #[serde(default)]
    pub epoch_ticks_per_call: Option<u64>,
    /// Overrides the memory ceiling applied to each guest store, in bytes.
    #[serde(default)]
    pub max_memory_bytes: Option<usize>,
}

impl From<&BudgetOverrides> for trogon_decider_sim::BudgetOverrides {
    fn from(value: &BudgetOverrides) -> Self {
        Self {
            fuel_per_call: value.fuel_per_call,
            epoch_ticks_per_call: value.epoch_ticks_per_call,
            max_memory_bytes: value.max_memory_bytes,
        }
    }
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
    /// `when`, and `then.events` value into wire form against `registry`.
    pub fn to_ir(&self, registry: &TypeRegistry) -> Result<ScenarioIr> {
        let mut ir = ScenarioIr::new(self.name.clone());
        ir.budget = self.budget.as_ref().map(trogon_decider_sim::BudgetOverrides::from);
        for value in &self.given {
            ir.given.push(codec::json_any_to_envelope(registry, value)?);
        }
        for (when, then) in self.steps()? {
            let when = codec::json_any_to_command(registry, when)?;
            let expect = match then {
                Then::Events { events } => {
                    let events = events
                        .iter()
                        .map(|value| codec::json_any_to_envelope(registry, value))
                        .collect::<Result<Vec<_>>>()?;
                    ExpectedOutcome::Events(events)
                }
                Then::Rejected { rejected: true } => ExpectedOutcome::Rejected,
                Then::Rejected { rejected: false } => ExpectedOutcome::Accepted,
                Then::Error { error } => ExpectedOutcome::Error(error.expected()?),
                Then::Trap { trap: true } => {
                    if self.budget.is_none() {
                        bail!(
                            "scenario '{}': `then.trap: true` requires a scenario-level `budget` override that \
                             starves the resource under test; under the default production budget the guest \
                             will not trap and the step reports a plain expectation mismatch instead",
                            self.name
                        );
                    }
                    ExpectedOutcome::Trap
                }
                Then::Trap { trap: false } => bail!(
                    "scenario '{}': `then.trap: false` is not a meaningful expectation; use \
                     `then.rejected`, `then.events`, or `then.error` instead",
                    self.name
                ),
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
    /// The guest call must trap: a Wasmtime-level fault such as fuel exhaustion, an expired
    /// epoch deadline, or a memory ceiling, rather than a decider-level outcome. Always paired
    /// with a scenario-level `budget` override that starves the resource under test.
    Trap {
        /// Whether the guest call must trap. Must be `true`; `false` is rejected in
        /// [`Scenario::to_ir`] since it is not a meaningful expectation on its own.
        /// [`Scenario::to_ir`] also rejects `true` without the paired `budget`, since
        /// nothing starves the guest under the default production budget.
        trap: bool,
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
