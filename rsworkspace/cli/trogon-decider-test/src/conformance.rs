//! Running a YAML suite against a compiled component.
//!
//! In the library rather than in `decider-test`'s `main` because the gate has a
//! second caller: `decider-publish` runs the same suite before a component
//! enters the module store, per
//! [ADR#0058](../../../../docs/adr/0058-decider-module-distribution.md). A
//! publisher that re-implemented the run would be a second definition of what
//! "conformant" means, and the two would drift.

use std::collections::BTreeSet;
use std::str::FromStr;

use anyhow::{Result, bail};
use trogon_decider_sim::{ExpectedOutcome, ScenarioIr, SimHost};

use crate::codec;
use crate::codec::{any_type_url, normalize_type_url};
use crate::{Suite, Then};

/// How a run reports each scenario.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// One `PASS`/`FAIL` line per scenario, for a person.
    #[default]
    Human,
    /// Test Anything Protocol, for a CI collector.
    Tap,
}

impl FromStr for OutputFormat {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "human" => Ok(Self::Human),
            "tap" => Ok(Self::Tap),
            other => bail!("unknown format '{other}', expected human or tap"),
        }
    }
}

/// Whether a declared command or event with no scenario covering it fails the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Strictness {
    /// A coverage gap fails the run.
    #[default]
    Strict,
    /// A coverage gap is reported and tolerated.
    Lenient,
}

impl Strictness {
    const fn is_strict(self) -> bool {
        matches!(self, Self::Strict)
    }

    const fn level(self) -> &'static str {
        if self.is_strict() { "error" } else { "warning" }
    }
}

/// Runs every scenario in `suite` against `wasm_bytes` and reports coverage.
///
/// The component's own declared module name has to be the suite's name: a
/// suite that runs green against a component it was not written for proves
/// nothing about either.
pub fn run_suite(wasm_bytes: &[u8], suite: &Suite, format: OutputFormat, strictness: Strictness) -> Result<()> {
    let host = SimHost::load(wasm_bytes)?;
    let descriptor = host.instantiate(())?.descriptor()?;
    if descriptor.name != suite.suite {
        bail!(
            "suite '{}' does not match the component's declared module '{}'",
            suite.suite,
            descriptor.name
        );
    }
    let module_name = descriptor.name;
    let registry = codec::type_registry(&module_name)?;
    let declared_commands = descriptor
        .commands
        .into_iter()
        .map(|spec| spec.command_type)
        .collect::<BTreeSet<_>>();
    let declared_events = codec::declared_events(&module_name)?
        .iter()
        .copied()
        .map(normalize_type_url)
        .collect::<BTreeSet<_>>();

    let mut exercised_commands = BTreeSet::new();
    let mut exercised_events = BTreeSet::new();
    let mut failures = 0usize;

    for scenario in &suite.scenarios {
        let steps = scenario.steps()?;

        for value in &scenario.given {
            exercised_events.insert(any_type_url(value)?);
        }
        for (when, then) in &steps {
            exercised_commands.insert(any_type_url(when)?);
            if let Then::Events { events } = then {
                for value in events {
                    exercised_events.insert(any_type_url(value)?);
                }
            }
        }

        let outcome = scenario
            .to_ir(registry)
            .and_then(|ir| run_scenario(&host, wasm_bytes, &ir));
        match outcome {
            Ok(()) => {
                if matches!(format, OutputFormat::Tap) {
                    println!("ok {} - {}", suite.suite, scenario.name);
                } else {
                    println!("PASS {}", scenario.name);
                }
            }
            Err(error) => {
                failures += 1;
                if matches!(format, OutputFormat::Tap) {
                    println!("not ok {} - {}: {error:#}", failures, scenario.name);
                } else {
                    eprintln!("FAIL {}: {error:#}", scenario.name);
                }
            }
        }
    }

    let command_gaps = report_coverage_gaps(&declared_commands, &exercised_commands, "command", strictness);
    let event_gaps = report_coverage_gaps(&declared_events, &exercised_events, "event", strictness);
    if strictness.is_strict() && (command_gaps > 0 || event_gaps > 0) {
        bail!("{command_gaps} declared command(s) and {event_gaps} declared event(s) have zero scenario coverage");
    }

    if failures > 0 {
        bail!("{failures} scenario(s) failed");
    }
    Ok(())
}

/// Reports every `declared` type with zero coverage in `exercised` and returns
/// how many gaps were found. The caller decides whether a nonzero strict-mode
/// count fails the run, so both the command and event checks always run and
/// report in full before the run bails.
pub fn report_coverage_gaps(
    declared: &BTreeSet<String>,
    exercised: &BTreeSet<String>,
    kind: &str,
    strictness: Strictness,
) -> usize {
    let gaps: Vec<&String> = declared.difference(exercised).collect();
    for gap in &gaps {
        eprintln!(
            "{level}: declared {kind} never exercised in any scenario: {gap}",
            level = strictness.level()
        );
    }
    gaps.len()
}

/// Runs one converted scenario. A scenario with a `budget` override loads a scenario-scoped
/// [`SimHost`] under that fault-injection budget instead of `host`'s default production budget,
/// since a starved fuel or epoch budget can trap as early as component instantiation, before the
/// scenario's own steps get a chance to run. A single-step scenario whose sole expectation is a
/// trap counts an instantiate-time trap as the scenario passing, mirroring how `SimScenario::run`
/// already treats a trap during `decide` as satisfying `.then_trap()`.
fn run_scenario(host: &SimHost, wasm_bytes: &[u8], ir: &ScenarioIr) -> Result<()> {
    let Some(budget) = ir.budget else {
        let mut instance = host.instantiate(())?;
        return ir.to_sim_scenario().run(&mut instance).map_err(anyhow::Error::new);
    };

    let config = budget.apply(host.config());
    let scenario_host = SimHost::load_with_config(wasm_bytes, config)?;
    match scenario_host.instantiate(()) {
        Ok(mut instance) => ir.to_sim_scenario().run(&mut instance).map_err(anyhow::Error::new),
        Err(error) if expects_trap(ir) && error.is_trap() => Ok(()),
        Err(error) => Err(anyhow::Error::new(error)),
    }
}

/// Returns whether `ir` is a single step whose sole expectation is a trap, the only shape under
/// which an instantiate-time trap (rather than a `decide`-time one) can stand in for the
/// scenario's expectation.
fn expects_trap(ir: &ScenarioIr) -> bool {
    matches!(ir.steps.as_slice(), [step] if matches!(step.expect, ExpectedOutcome::Trap))
}

#[cfg(test)]
mod tests;
