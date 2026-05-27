use std::error::Error;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use trogon_agent_registry_controller::{ControllerConfig, run_controller};

type BoxError = Box<dyn Error + Send + Sync>;

fn env_var_is_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .try_init();
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    init_tracing();
    let mut config = ControllerConfig::parse();
    if !config.auto_create_bucket {
        config.auto_create_bucket = env_var_is_truthy("TROGON_REGISTRY_AUTOCREATE");
    }
    run_controller(config).await
}
