#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod fixture;
mod lint;
mod method;
mod runner;

use std::path::{Path, PathBuf};
use std::process;

use a2a_gateway::policy::Tier2CompiledBundle;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::fixture::FixtureSuite;
use crate::runner::OutputFormat;

#[derive(Parser)]
#[command(
    name = "tier2-cel-test",
    about = "Fixture-replay and lint toolchain for a2a-gateway Tier-2 CEL policy bundles"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Replay a YAML/JSON fixture suite against its `.cel` bundle and
    /// compare each fixture's expected verdict to the real evaluator's
    /// decision.
    Test {
        /// Output format (`human` or `tap`)
        #[arg(long, default_value = "human")]
        format: String,

        /// Fixture suite file (YAML or JSON)
        suite: PathBuf,
    },
    /// Lint every `.cel` file in a bundle directory for unbound variable
    /// references, duplicate rules, contradictory rules, and rules that
    /// can never be reached.
    Lint {
        /// Bundle directory containing `.cel` files
        bundle: PathBuf,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Test { format, suite } => run_test(&suite, &format),
        Command::Lint { bundle } => run_lint(&bundle),
    }
}

fn run_test(suite_path: &Path, format_raw: &str) -> Result<()> {
    let format = parse_output_format(format_raw)?;
    let suite: FixtureSuite = load_suite(suite_path)?;

    let suite_dir = suite_path.parent().unwrap_or_else(|| Path::new("."));
    let bundle_dir = suite_dir.join(&suite.bundle);
    let bundle = Tier2CompiledBundle::load_from_dir(&bundle_dir)
        .map_err(|err| anyhow::anyhow!("load cel bundle {}: {err}", bundle_dir.display()))?;

    let results = runner::run_suite(bundle, &suite.fixtures);
    let failures = runner::report(&suite.suite, &results, format);

    if failures > 0 {
        bail!("{failures} fixture(s) failed");
    }
    Ok(())
}

fn run_lint(bundle_dir: &Path) -> Result<()> {
    let report = lint::lint_bundle(bundle_dir).with_context(|| format!("lint bundle {}", bundle_dir.display()))?;

    for finding in &report.findings {
        println!("{finding}");
    }

    let error_count = report.errors().count();
    let warning_count = report.warnings().count();
    if error_count == 0 && warning_count == 0 {
        println!("no issues found");
    }

    if report.has_errors() {
        bail!("{error_count} error(s), {warning_count} warning(s) found");
    }
    Ok(())
}

fn load_suite(path: &Path) -> Result<FixtureSuite> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => serde_json::from_str(&raw).with_context(|| format!("parse {} as JSON", path.display())),
        _ => serde_yaml::from_str(&raw).with_context(|| format!("parse {} as YAML", path.display())),
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
mod tests;
