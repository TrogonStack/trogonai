#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use std::fs;
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use clap::Parser;
use trogon_decider_test::Suite;
use trogon_decider_test::conformance::{OutputFormat, Strictness, run_suite};

#[derive(Parser)]
#[command(
    name = "decider-test",
    about = "Run YAML decider conformance suites against a WASM component"
)]
struct Args {
    /// Output format (`human` or `tap`)
    #[arg(long, default_value = "human")]
    format: OutputFormat,

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

#[cfg_attr(
    dylint_lib = "trogon_lints",
    allow(
        debug_remnants,
        reason = "the CLI exits before a subscriber is installed, so stderr is the only channel left to report on"
    )
)]
fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("error: {error:#}");
        process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let wasm_bytes = fs::read(&args.wasm).with_context(|| format!("read {}", args.wasm.display()))?;
    let suite =
        Suite::from_yaml(&fs::read_to_string(&args.suite).with_context(|| format!("read {}", args.suite.display()))?)?;
    let strictness = if args.no_strict {
        Strictness::Lenient
    } else {
        Strictness::Strict
    };

    run_suite(&wasm_bytes, &suite, args.format, strictness)
}

#[cfg(test)]
mod tests;
