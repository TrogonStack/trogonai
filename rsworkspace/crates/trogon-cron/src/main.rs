use std::env;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let nats_url =
        env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());

    tracing::info!(nats_url = %nats_url, "Connecting to NATS");

    let nats = async_nats::connect(&nats_url)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to connect to NATS");
            std::process::exit(1);
        });

    tracing::info!("Starting CRON scheduler");

    if let Err(e) = trogon_cron::Scheduler::new(nats).run().await {
        tracing::error!(error = %e, "Scheduler exited with error");
        std::process::exit(1);
    }
}
