#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! `trogon-secrets-service` binary.

use std::error::Error;
use std::process::ExitCode;

use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_secrets_service::{ReleaseSources, SecretsService, ServiceArgs, ServiceConfig, error_chain};
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

    match serve().await {
        Ok(()) => {
            tracing::info!("trogon-secrets-service stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(error = %error_chain(error.as_ref()), "trogon-secrets-service stopped on an error");
            ExitCode::FAILURE
        }
    }
}

/// Resolves the configuration, loads what it points at, and runs the service
/// until a shutdown signal.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn serve() -> Result<(), Box<dyn Error>> {
    let args = ServiceArgs::parse();
    let config = ServiceConfig::resolve(&args, &SystemEnv, &SystemFs)?;
    let sources = ReleaseSources::load(&config, &SystemEnv, &SystemFs)?;

    let shutdown = CancellationToken::new();
    let on_signal = shutdown.clone();
    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        on_signal.cancel();
    });
    SecretsService::new(config, sources).run(shutdown).await?;
    Ok(())
}
