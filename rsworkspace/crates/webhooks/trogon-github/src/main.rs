use std::time::Duration;
use trogon_github::{GithubConfig, serve};
use trogon_nats::connect;
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = GithubConfig::from_env(&SystemEnv);
    let nats = connect(&config.nats, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    serve(config, nats).await.expect("Server failed");
}
