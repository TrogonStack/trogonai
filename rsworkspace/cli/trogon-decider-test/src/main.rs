#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result, bail};
use clap::Parser;
use trogon_decider_sim::{SimHost, SimInstance};
use trogon_decider_test::codec::{any_type_url, normalize_type_url};
use trogon_decider_test::{Scenario, Suite, Then};

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
    let suite =
        Suite::from_yaml(&fs::read_to_string(&args.suite).with_context(|| format!("read {}", args.suite.display()))?)?;

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

        match run_scenario(&mut instance, scenario) {
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

fn run_scenario(instance: &mut SimInstance<()>, scenario: &Scenario) -> Result<()> {
    let ir = scenario.to_ir()?;
    ir.to_sim_scenario().run(instance).map_err(anyhow::Error::new)
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
