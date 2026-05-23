use clap::Parser;
use tracing::error;

use a2a_nats_discovery::{Args, run};

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
        error!(error = %e, "A2A NATS discovery service failed");
        std::process::exit(1);
    }
}
