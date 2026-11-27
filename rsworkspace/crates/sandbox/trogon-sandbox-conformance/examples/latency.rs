//! Prints a latency table for one worker: create, first exec, file write, file read, snapshot and delete, as p50 and
//! p95 over the iterations.
//!
//! ```text
//! # the in-process memory worker on a NATS server in a container (needs Docker)
//! cargo run --release -p trogon-sandbox-conformance --example latency
//!
//! # a worker that is already running against a NATS server
//! cargo run --release -p trogon-sandbox-conformance --example latency -- \
//!   --nats-url nats://127.0.0.1:4222 --provider local_docker --image my-image:tag
//! ```

use std::error::Error;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{Target, measure_latency, percentile, render_latency, snapshot_push_target};
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_memory_worker::{MemoryWorker, MemoryWorkerConfig};

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        constant_outside_constants_module,
        reason = "the usage text is private to this example"
    )
)]
const USAGE: &str = "usage: latency [--nats-url URL] [--provider NAME] [--image IMAGE] \
[--iterations N] [--snapshot-registry HOST --snapshot-repository REPO]

Without --nats-url the in-process memory worker runs on a NATS server in a
container. With it, the worker for --provider (a snake_case provider name such
as local_docker or daytona) must already be attached to that server. The
snapshot step runs only with --snapshot-registry.";

/// Why the command line was refused.
#[derive(Debug, thiserror::Error)]
enum ArgsError {
    #[error("{0} needs a value")]
    MissingValue(String),
    #[error("unknown provider {0}")]
    UnknownProvider(String),
    #[error("--iterations takes a number of at least 1, not {0}")]
    Iterations(String),
    #[error("unknown flag {0}")]
    UnknownFlag(String),
}

struct Args {
    nats_url: Option<String>,
    provider: ProviderKind,
    image: Option<String>,
    iterations: usize,
    registry: Option<String>,
    repository: String,
}

impl Args {
    fn parse(mut argv: impl Iterator<Item = String>) -> Result<Self, ArgsError> {
        let mut args = Self {
            nats_url: None,
            provider: ProviderKind::Mock,
            image: None,
            iterations: 10,
            registry: None,
            repository: "latency/snapshots".to_owned(),
        };
        while let Some(flag) = argv.next() {
            let mut value = || argv.next().ok_or_else(|| ArgsError::MissingValue(flag.clone()));
            match flag.as_str() {
                "--nats-url" => args.nats_url = Some(value()?),
                "--provider" => {
                    let name = value()?;
                    args.provider = serde_json::from_value(serde_json::Value::String(name.clone()))
                        .map_err(|_| ArgsError::UnknownProvider(name))?;
                }
                "--image" => args.image = Some(value()?),
                "--iterations" => {
                    let raw = value()?;
                    args.iterations = raw.parse().ok().filter(|n| *n >= 1).ok_or(ArgsError::Iterations(raw))?;
                }
                "--snapshot-registry" => args.registry = Some(value()?),
                "--snapshot-repository" => args.repository = value()?,
                _ => return Err(ArgsError::UnknownFlag(flag)),
            }
        }
        Ok(args)
    }
}

#[tokio::main]
async fn main() -> Result<ExitCode, Box<dyn Error>> {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            writeln!(std::io::stderr(), "{error}\n\n{USAGE}")?;
            return Ok(ExitCode::from(2));
        }
    };
    let (server, client) = match &args.nats_url {
        Some(url) => (None, async_nats::connect(url).await?),
        None => {
            let server = CoreTestServer::start().await;
            let client = async_nats::connect(server.address()).await?;
            (Some(server), client)
        }
    };
    let worker = match server {
        Some(_) => Some(MemoryWorker::start(client.clone(), MemoryWorkerConfig::default()).await?),
        None => None,
    };
    let mut target = Target::new(client, args.provider);
    target.timeout = Duration::from_secs(120);
    if let Some(image) = args.image {
        target.image = image;
    }
    let push = args
        .registry
        .as_deref()
        .map(|registry| snapshot_push_target(registry, &args.repository));
    let report = measure_latency(&target, args.iterations, push.as_ref()).await;
    writeln!(std::io::stdout(), "{}", render_latency(std::slice::from_ref(&report)))?;
    // The tail the table leaves out, for comparisons that need it.
    for step in &report.steps {
        let mut sorted = step.samples.clone();
        sorted.sort();
        if let Some(p99) = percentile(&sorted, 99.0) {
            writeln!(std::io::stdout(), "p99 {}: {:.1} ms", step.step, p99.as_secs_f64() * 1000.0)?;
        }
    }
    if let Some(worker) = worker {
        worker.shutdown().await;
    }
    drop(server);
    Ok(ExitCode::SUCCESS)
}
