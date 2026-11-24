#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! `trogon-egress-proxy` binary.

use std::error::Error;
use std::process::ExitCode;

use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_egress_proxy::{Args, Config, start};
use trogon_telemetry::ServiceName;

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() -> ExitCode {
    trogon_telemetry::init_logger(
        ServiceName::TrogonEgressProxy,
        [],
        &trogon_std::env::SystemEnv,
        &trogon_std::fs::SystemFs,
    );

    let code = run().await;
    if let Err(error) = trogon_telemetry::shutdown_otel() {
        tracing::error!(%error, "trogon-egress-proxy telemetry shutdown failed");
    }
    code
}

#[cfg_attr(coverage_nightly, coverage(off))]
async fn run() -> ExitCode {
    let args = Args::parse();
    let readiness_listen = args.readiness_listen;
    let config = match Config::try_from(args) {
        Ok(config) => config,
        Err(error) => return fail(&error),
    };
    let server = match start(config).await {
        Ok(server) => server,
        Err(error) => return fail(&error),
    };
    let listener = match tokio::net::TcpListener::bind(readiness_listen).await {
        Ok(listener) => listener,
        Err(error) => return fail(&error),
    };
    let readiness = server.readiness();

    let shutdown = CancellationToken::new();
    let on_signal = shutdown.clone();
    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        on_signal.cancel();
    });
    tokio::join!(server.run(shutdown.clone()), readiness.serve(listener, shutdown));
    ExitCode::SUCCESS
}

fn fail(error: &dyn Error) -> ExitCode {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    tracing::error!(error = %message, "trogon-egress-proxy cannot start");
    ExitCode::FAILURE
}
