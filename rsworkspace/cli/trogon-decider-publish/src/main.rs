#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use std::fs;
use std::path::PathBuf;
use std::process;

use anyhow::{Context, Result};
use async_nats::jetstream;
use clap::Parser;
use trogon_decider_nats_server::constants::{DEFAULT_MODULE_BUCKET, NATS_CONNECT_TIMEOUT};
use trogon_decider_publish::{PublishRequest, publish};
use trogon_decider_test::Suite;
use trogon_decider_test::conformance::OutputFormat;
use trogon_nats::NatsConfig;
use trogon_std::env::SystemEnv;

#[derive(Parser)]
#[command(
    name = "decider-publish",
    about = "Publish a decider component into the module store, gated on its conformance suite"
)]
struct Args {
    /// Object store bucket to publish into
    #[arg(long, default_value = DEFAULT_MODULE_BUCKET)]
    bucket: String,

    /// Output format for the conformance run (`human` or `tap`)
    #[arg(long, default_value = "human")]
    format: OutputFormat,

    /// Compiled decider component
    wasm: PathBuf,

    /// YAML conformance suite the component must pass
    suite: PathBuf,
}

#[tokio::main]
#[cfg_attr(
    dylint_lib = "trogon_lints",
    allow(
        debug_remnants,
        reason = "the CLI exits before a subscriber is installed, so stderr is the only channel left to report on"
    )
)]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() {
    if let Err(error) = run(Args::parse()).await {
        eprintln!("error: {error:#}");
        process::exit(1);
    }
}

#[cfg_attr(
    dylint_lib = "trogon_lints",
    allow(
        debug_remnants,
        reason = "the published reference on stdout is this CLI's result, which a caller pipes onward"
    )
)]
async fn run(args: Args) -> Result<()> {
    let component = fs::read(&args.wasm).with_context(|| format!("read {}", args.wasm.display()))?;
    let suite =
        Suite::from_yaml(&fs::read_to_string(&args.suite).with_context(|| format!("read {}", args.suite.display()))?)?;

    let client = trogon_nats::connect(&NatsConfig::from_env(&SystemEnv), NATS_CONNECT_TIMEOUT).await?;
    let reference = publish(
        &jetstream::new(client),
        &PublishRequest {
            component: &component,
            suite: &suite,
            bucket: &args.bucket,
            format: args.format,
        },
    )
    .await?;

    println!("published {reference} to {}", args.bucket);
    Ok(())
}
