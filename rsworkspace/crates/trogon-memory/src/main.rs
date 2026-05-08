use std::time::Duration;

use trogon_nats::connect;
use trogon_std::env::SystemEnv;
use trogon_memory::{
    AnthropicMemoryProvider, DreamingConfig, DreamingService, Dreamer,
    provision_kv, provision_stream,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = DreamingConfig::from_env(&SystemEnv);
    let nats = connect(&config.nats, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let js = async_nats::jetstream::new(nats);

    provision_stream(&js).await.expect("Failed to provision SESSION_DREAMS stream");
    let kv = provision_kv(&js).await.expect("Failed to provision SESSION_MEMORIES bucket");

    let provider = AnthropicMemoryProvider::new(config.llm);
    let dreamer = Dreamer::new(provider, kv);
    let service = DreamingService::new(js, config.consumer_name, dreamer);

    service.run().await.expect("Dreaming service failed");
}
