mod codec;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;
use trogon_decider_sim::{SimHost, SimScenario};

use crate::codec::{any_type_url, json_any_to_command, json_any_to_envelope};

#[derive(Parser)]
#[command(
    name = "decider-test",
    about = "Run YAML decider conformance suites against a WASM component"
)]
struct Args {
    /// Output format (`human` or `tap`)
    #[arg(long, default_value = "human")]
    format: String,

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
    scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    #[serde(default)]
    given: Vec<serde_json::Value>,
    when: serde_json::Value,
    then: Then,
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
    let declared = host
        .instantiate(())?
        .descriptor()?
        .commands
        .into_iter()
        .map(|spec| spec.command_type)
        .collect::<BTreeSet<_>>();
    let mut exercised = BTreeSet::new();

    let mut failures = 0usize;
    for scenario in &suite.scenarios {
        // Fresh component per scenario so guest-global state cannot leak between runs.
        let mut instance = host.instantiate(())?;
        let when_type = any_type_url(&scenario.when)?;
        exercised.insert(when_type.clone());
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

    for command_type in declared.difference(&exercised) {
        eprintln!("warning: declared command never exercised as when: {command_type}");
    }

    if failures > 0 {
        bail!("{failures} scenario(s) failed");
    }
    Ok(())
}

fn run_scenario(instance: &mut trogon_decider_sim::SimInstance<()>, scenario: &Scenario) -> Result<()> {
    let given = scenario
        .given
        .iter()
        .map(json_any_to_envelope)
        .collect::<Result<Vec<_>>>()?;
    let when = json_any_to_command(&scenario.when)?;

    match &scenario.then {
        Then::Events { events } => {
            let expected = events.iter().map(json_any_to_envelope).collect::<Result<Vec<_>>>()?;
            SimScenario::new()
                .given(given)
                .when(when)
                .then_events(expected)
                .run(instance)
                .map_err(|error| anyhow::anyhow!(error))
        }
        Then::Rejected { rejected } => {
            let scenario = SimScenario::new().given(given).when(when);
            let scenario = if *rejected {
                scenario.then_rejected()
            } else {
                scenario.then_accepted()
            };
            scenario.run(instance).map_err(|error| anyhow::anyhow!(error))
        }
        Then::Error { error } => SimScenario::new()
            .given(given)
            .when(when)
            .then_error(error.expected()?)
            .run(instance)
            .map_err(|err| anyhow::anyhow!(err)),
    }
}

fn parse_output_format(raw: &str) -> Result<OutputFormat> {
    match raw {
        "human" => Ok(OutputFormat::Human),
        "tap" => Ok(OutputFormat::Tap),
        other => bail!("unknown format '{other}', expected human or tap"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_expectation(yaml: &str) -> String {
        match serde_yaml::from_str::<Then>(yaml) {
            Ok(Then::Error { error }) => error.expected().unwrap_or_default(),
            _ => String::new(),
        }
    }

    #[test]
    fn then_error_accepts_structured_code() {
        assert_eq!(error_expectation("error:\n  code: already-exists\n"), "already-exists");
    }

    #[test]
    fn then_error_accepts_plain_string() {
        assert_eq!(error_expectation("error: already-exists\n"), "already-exists");
    }
}
