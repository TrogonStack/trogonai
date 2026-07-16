#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod codec;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;
use trogon_decider_sim::{SimHost, SimScenario};

use crate::codec::{any_type_url, json_any_to_command, json_any_to_envelope, normalize_type_url};

#[derive(Parser)]
#[command(
    name = "decider-test",
    about = "Run YAML decider conformance suites against a WASM component"
)]
struct Args {
    /// Output format (`human` or `tap`)
    #[arg(long, default_value = "human")]
    format: String,

    /// Downgrade zero-coverage declared commands/events from a failure to a
    /// warning. By default a declared command or event with no coverage
    /// across every scenario fails the run.
    #[arg(long)]
    no_strict: bool,

    /// Compiled decider component
    wasm: PathBuf,

    /// YAML test suite
    suite: PathBuf,
}

#[derive(Clone, Copy, Default)]
enum OutputFormat {
    #[default]
    Human,
    Tap,
}

#[derive(Debug, Deserialize)]
struct Suite {
    suite: String,
    /// Type URLs (bare or `type.googleapis.com/`-prefixed) of every event the
    /// decider under test can produce, asserted by the suite author. Compared
    /// against every event actually referenced across all scenarios' `given`
    /// and `then.events` entries for the strict coverage check, since neither
    /// the WIT descriptor nor the proto type registry can distinguish "events
    /// this decider emits" from unrelated message and helper types.
    #[serde(default)]
    events: Vec<String>,
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    #[serde(default)]
    given: Vec<serde_json::Value>,
    #[serde(default)]
    when: Option<serde_json::Value>,
    #[serde(default)]
    then: Option<Then>,
    /// An ordered `when`/`then` sequence run against a single open session,
    /// each step's emitted events folded in before the next step's command is
    /// decided. Mutually exclusive with the single `when`/`then` shape.
    #[serde(default)]
    steps: Option<Vec<Step>>,
}

#[derive(Debug, Deserialize)]
struct Step {
    when: serde_json::Value,
    then: Then,
}

impl Scenario {
    /// Returns this scenario's ordered when/then steps, normalizing the
    /// legacy single `when`/`then` shape into a one-element step list.
    ///
    /// Exactly one of `steps` (nonempty) or `when`+`then` must be present;
    /// any other combination is a malformed scenario.
    fn steps(&self) -> Result<Vec<(&serde_json::Value, &Then)>> {
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
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum Then {
    Events { events: Vec<serde_json::Value> },
    Error { error: ErrorExpectation },
    Rejected { rejected: bool },
}

/// `then.error` accepts either a bare string or the documented `{ code, message }`
/// object; both are matched against the domain error's code or message.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ErrorExpectation {
    Structured {
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        message: Option<String>,
    },
    Plain(String),
}

impl ErrorExpectation {
    fn expected(&self) -> Result<String> {
        match self {
            Self::Plain(value) => Ok(value.clone()),
            Self::Structured { code, message } => code
                .clone()
                .or_else(|| message.clone())
                .context("then.error requires a code or message"),
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let output_format = parse_output_format(&args.format)?;
    let wasm_bytes = fs::read(&args.wasm).with_context(|| format!("read {}", args.wasm.display()))?;
    let suite: Suite = serde_yaml::from_str(
        &fs::read_to_string(&args.suite).with_context(|| format!("read {}", args.suite.display()))?,
    )?;

    let host = SimHost::load(&wasm_bytes)?;
    let declared_commands = host
        .instantiate(())?
        .descriptor()?
        .commands
        .into_iter()
        .map(|spec| spec.command_type)
        .collect::<BTreeSet<_>>();
    let declared_events = suite
        .events
        .iter()
        .map(|type_url| normalize_type_url(type_url))
        .collect::<BTreeSet<_>>();

    let mut exercised_commands = BTreeSet::new();
    let mut exercised_events = BTreeSet::new();
    let mut failures = 0usize;

    for scenario in &suite.scenarios {
        let mut instance = host.instantiate(())?;
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

        match run_scenario(&mut instance, &scenario.given, &steps) {
            Ok(()) => {
                if matches!(output_format, OutputFormat::Tap) {
                    println!("ok {} - {}", suite.suite, scenario.name);
                } else {
                    println!("PASS {}", scenario.name);
                }
            }
            Err(error) => {
                failures += 1;
                if matches!(output_format, OutputFormat::Tap) {
                    println!("not ok {} - {}: {error:#}", failures, scenario.name);
                } else {
                    eprintln!("FAIL {}: {error:#}", scenario.name);
                }
            }
        }
    }

    let command_gaps = report_coverage_gaps(&declared_commands, &exercised_commands, "command", args.no_strict);
    let event_gaps = report_coverage_gaps(&declared_events, &exercised_events, "event", args.no_strict);
    if !args.no_strict && (command_gaps > 0 || event_gaps > 0) {
        bail!("{command_gaps} declared command(s) and {event_gaps} declared event(s) have zero scenario coverage");
    }

    if failures > 0 {
        bail!("{failures} scenario(s) failed");
    }
    Ok(())
}

/// Reports every `declared` type with zero coverage in `exercised`, at
/// `warning` level under `--no-strict` and `error` level otherwise, and
/// returns how many gaps were found. The caller decides whether a nonzero
/// strict-mode count fails the run, so both the command and event checks
/// always run and report in full before the run bails.
fn report_coverage_gaps(
    declared: &BTreeSet<String>,
    exercised: &BTreeSet<String>,
    kind: &str,
    no_strict: bool,
) -> usize {
    let gaps: Vec<&String> = declared.difference(exercised).collect();
    let level = if no_strict { "warning" } else { "error" };
    for gap in &gaps {
        eprintln!("{level}: declared {kind} never exercised in any scenario: {gap}");
    }
    gaps.len()
}

fn run_scenario(
    instance: &mut trogon_decider_sim::SimInstance<()>,
    given: &[serde_json::Value],
    steps: &[(&serde_json::Value, &Then)],
) -> Result<()> {
    let given = given.iter().map(json_any_to_envelope).collect::<Result<Vec<_>>>()?;
    let mut sim = SimScenario::new().given(given);

    for (when, then) in steps.iter().copied() {
        let command = json_any_to_command(when)?;
        sim = sim.when(command);
        sim = match then {
            Then::Events { events } => {
                let expected = events.iter().map(json_any_to_envelope).collect::<Result<Vec<_>>>()?;
                sim.then_events(expected)
            }
            Then::Rejected { rejected } => {
                if *rejected {
                    sim.then_rejected()
                } else {
                    sim.then_accepted()
                }
            }
            Then::Error { error } => sim.then_error(error.expected()?),
        };
    }

    sim.run(instance).map_err(anyhow::Error::new)
}

fn parse_output_format(raw: &str) -> Result<OutputFormat> {
    match raw {
        "human" => Ok(OutputFormat::Human),
        "tap" => Ok(OutputFormat::Tap),
        other => bail!("unknown format '{other}', expected human or tap"),
    }
}

#[cfg(test)]
mod tests;
