//! Sandbox broker service.

use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use trogon_sandbox_broker::cli::Cli;
use trogon_sandbox_broker::serve::{self, SERVICE_NAME};
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let _guard = trogon_sandbox_observability::init_tracing(SERVICE_NAME, cli.observability.otlp_endpoint.as_deref())?;
    let shutdown = CancellationToken::new();
    tokio::spawn(cancel_on_signal(shutdown.clone()));
    serve::run(cli, &SystemEnv, shutdown).await?;
    Ok(())
}

/// Starts a graceful shutdown on ctrl-c or, on Unix, SIGTERM.
async fn cancel_on_signal(shutdown: CancellationToken) {
    tokio::select! {
        () = interrupt() => {}
        () = terminate() => {}
    }
    shutdown.cancel();
}

async fn interrupt() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        warn!(error = %error, "cannot listen for ctrl-c");
        std::future::pending::<()>().await;
    }
}

#[cfg(unix)]
async fn terminate() {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(mut signal) => {
            signal.recv().await;
        }
        Err(error) => {
            warn!(error = %error, "cannot listen for SIGTERM");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(not(unix))]
async fn terminate() {
    std::future::pending::<()>().await;
}
