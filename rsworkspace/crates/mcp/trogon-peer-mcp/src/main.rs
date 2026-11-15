mod telemetry;

use std::process::ExitCode;

use trogon_peer_mcp::{RuntimeConfig, RuntimeError, run_stdio};
use trogon_std::env::SystemEnv;
use trogon_std::signal::shutdown_signal;

#[tokio::main]
async fn main() -> ExitCode {
    telemetry::install();
    let config = match RuntimeConfig::from_env(&SystemEnv) {
        Ok(config) => config,
        Err(_) => {
            telemetry::invalid_settings();
            return ExitCode::FAILURE;
        }
    };
    match run_stdio(config, shutdown_signal()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            match error {
                RuntimeError::Stdio(_)
                | RuntimeError::StartupDeadline
                | RuntimeError::Connect(_)
                | RuntimeError::ConnectReady(_)
                | RuntimeError::Configure(_)
                | RuntimeError::Initialize(_) => telemetry::startup_failure(),
                RuntimeError::Connector(_) | RuntimeError::Shutdown(_) | RuntimeError::ShutdownDeadline => {
                    telemetry::unconfirmed_shutdown()
                }
            }
            ExitCode::FAILURE
        }
    }
}
