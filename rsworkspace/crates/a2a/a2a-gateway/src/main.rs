//! `a2a-gateway` binary.
//!
//! The production main wires tracing + clap + the gateway runtime. Under
//! `cfg(coverage)` it collapses to a stub `fn main()` so the coverage build
//! still measures the binary target without hard-wiring a real tokio runtime
//! into the coverage harness. Matches the pattern used by `a2a-nats-server`.

#[cfg(not(coverage))]
use clap::Parser;
#[cfg(not(coverage))]
use tracing::error;

#[cfg(not(coverage))]
use a2a_gateway::{Args, run};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    if let Err(e) = run(args).await {
        error!(error = %e, "A2A gateway failed");
        std::process::exit(1);
    }
}

#[cfg(coverage)]
fn main() {}

#[cfg(test)]
mod tests;
