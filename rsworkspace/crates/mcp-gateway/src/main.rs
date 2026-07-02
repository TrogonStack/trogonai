#[cfg(not(coverage))]
use clap::Parser;
#[cfg(not(coverage))]
use tracing::error;

#[cfg(not(coverage))]
use mcp_gateway::{Args, runtime::run_with_args};

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

    if let Err(e) = run_with_args(args, &trogon_std::env::SystemEnv).await {
        error!(error = %e, "MCP gateway failed");
        std::process::exit(1);
    }
}

#[cfg(coverage)]
fn main() {}
