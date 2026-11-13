#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! `trogon-secrets-service` binary.

use std::error::Error;
use std::process::ExitCode;

use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_secrets_service::{ReleaseSources, SecretsService, ServiceArgs, ServiceConfig};
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = ServiceArgs::parse();
    let config = match ServiceConfig::resolve(&args, &SystemEnv, &SystemFs) {
        Ok(config) => config,
        Err(error) => return fail(&error),
    };
    let sources = match ReleaseSources::load(&config, &SystemEnv, &SystemFs) {
        Ok(sources) => sources,
        Err(error) => return fail(&error),
    };

    let shutdown = CancellationToken::new();
    let on_signal = shutdown.clone();
    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        on_signal.cancel();
    });
    match SecretsService::new(config, sources).run(shutdown).await {
        Ok(()) => {
            tracing::info!("trogon-secrets-service stopped");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

/// Logs `error` and every error under it, then exits non-zero. No error the
/// service returns quotes a secret.
fn fail(error: &dyn Error) -> ExitCode {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    tracing::error!(error = %message, "trogon-secrets-service stopped on an error");
    ExitCode::FAILURE
}
