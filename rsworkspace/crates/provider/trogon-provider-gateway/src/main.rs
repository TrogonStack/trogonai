#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod cli;
mod config;
mod constants;
mod error;
mod server;
mod telemetry;

use clap::Parser;
use trogon_std::env::SystemEnv;
use trogon_std::fs::SystemFs;

#[tokio::main]
#[cfg_attr(coverage_nightly, coverage(off))]
async fn main() -> Result<(), error::Error> {
    let cli = cli::Cli::parse();
    let config = config::load(&cli, &SystemEnv)?;
    trogon_std::tls::install_default_crypto_provider();

    trogon_telemetry::init_logger(
        trogon_telemetry::ServiceName::TrogonProviderGateway,
        [],
        &SystemEnv,
        &SystemFs,
    );

    let health = trogon_provider_gateway::Health::default();
    let result = match server::bind(config.bind_addr()).await {
        Ok(listener) => {
            server::serve(
                listener,
                trogon_provider_gateway::router(health.clone()),
                health,
                trogon_std::signal::shutdown_signal(),
            )
            .await
        }
        Err(error) => Err(error),
    };
    let shutdown = trogon_telemetry::shutdown_otel();
    if let Err(error) = &shutdown {
        tracing::error!(%error, "OpenTelemetry shutdown failed");
    }

    result?;
    shutdown?;
    Ok(())
}
