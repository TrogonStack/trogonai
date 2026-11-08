#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! `a2a-gateway` binary.

use clap::Parser;
use tracing::error;

use a2a_gateway::{Args, run};

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() {
    trogon_telemetry::init_logger(
        trogon_telemetry::ServiceName::A2aGateway,
        [],
        &trogon_std::env::SystemEnv,
        &trogon_std::fs::SystemFs,
    );

    let args = Args::parse();

    let result = run(args).await;
    if let Err(error) = trogon_telemetry::shutdown_otel() {
        error!(%error, "A2A gateway telemetry shutdown failed");
    }
    if let Err(e) = result {
        error!(error = %e, "A2A gateway failed");
        std::process::exit(1);
    }
}
