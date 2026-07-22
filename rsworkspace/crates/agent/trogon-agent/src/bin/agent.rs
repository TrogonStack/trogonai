//! Entry point for the `trogon-agent` binary.
//!
//! Reads configuration from environment variables and starts the NATS
//! subscriber loop.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let env = trogon_std::env::SystemEnv;
    let cfg = trogon_agent::AgentConfig::from_env(&env);

    if let Err(e) = trogon_agent::run(cfg).await {
        eprintln!("Agent error: {e}");
        std::process::exit(1);
    }
}
