#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[cfg(not(coverage))]
mod cli;
#[cfg(not(coverage))]
mod runtime_config;

#[cfg(not(coverage))]
use std::time::Duration;

#[cfg(not(coverage))]
use tracing::{error, info, warn};
#[cfg(not(coverage))]
use trogon_cron::CronController;
#[cfg(not(coverage))]
use trogon_nats::connect;
#[cfg(not(coverage))]
use trogon_std::args::{CliArgs, ParseArgs};
#[cfg(not(coverage))]
use trogon_std::env::SystemEnv;
#[cfg(not(coverage))]
use trogon_std::fs::SystemFs;

#[cfg(not(coverage))]
const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = CliArgs::<cli::Cli>::new().parse_args();

    trogon_telemetry::init_logger(trogon_telemetry::ServiceName::TrogonCron, [], &SystemEnv, &SystemFs);

    info!("trogon-cron starting");

    let result = run(cli).await;

    match &result {
        Ok(()) => info!("trogon-cron stopped"),
        Err(e) => error!(error = %e, "trogon-cron stopped with error"),
    }

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result
}

#[cfg(coverage)]
fn main() {}

#[cfg(not(coverage))]
async fn run(cli: cli::Cli) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = runtime_config::load_with_overrides(cli.runtime.config.as_deref(), &cli.runtime.nats)?;
    let nats = connect(&resolved.nats, NATS_CONNECT_TIMEOUT).await?;

    run_controller(nats).await
}

#[cfg(not(coverage))]
async fn run_controller(nats: async_nats::Client) -> Result<(), Box<dyn std::error::Error>> {
    let controller = CronController::from_nats(nats).await?;
    controller.run_until(shutdown_signal()).await?;
    Ok(())
}

#[cfg(not(coverage))]
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            warn!(error = %error, "Failed to install Ctrl+C handler");
            std::future::pending::<()>().await;
            return;
        }
        info!("Received SIGINT (Ctrl+C)");
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
                info!("Received SIGTERM");
            }
            Err(error) => {
                warn!(error = %error, "Failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
