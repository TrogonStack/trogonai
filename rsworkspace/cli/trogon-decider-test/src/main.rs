#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result, bail};
use clap::Parser;
use trogon_decider_sim::{ExpectedOutcome, ScenarioIr, SimHost};
use trogon_decider_test::codec;
use trogon_decider_test::codec::{any_type_url, normalize_type_url};
use trogon_decider_test::{Suite, Then};

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

#[derive(Debug, Clone, Copy, Default)]
enum OutputFormat {
    #[default]
    Human,
    Tap,
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("error: {error:#}");
        process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let output_format = parse_output_format(&args.format)?;
    let wasm_bytes = fs::read(&args.wasm).with_context(|| format!("read {}", args.wasm.display()))?;
    let suite =
        Suite::from_yaml(&fs::read_to_string(&args.suite).with_context(|| format!("read {}", args.suite.display()))?)?;

    let host = SimHost::load(&wasm_bytes)?;
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
            .and_then(|ir| run_scenario(&host, &wasm_bytes, &ir));
        match outcome {
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

fn parse_output_format(raw: &str) -> Result<OutputFormat> {
    match raw {
        "human" => Ok(OutputFormat::Human),
        "tap" => Ok(OutputFormat::Tap),
        other => bail!("unknown format '{other}', expected human or tap"),
    }
}

#[cfg(test)]
mod tests;
