use std::time::Duration;

use trogon_nats::connect;
use trogon_std::env::SystemEnv;
use trogon_memory::{
    AnthropicMemoryProvider, DreamingConfig, DreamingService, Dreamer,
    MemoryWriter, provision_kv, provision_stream, serve,
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

    let js = async_nats::jetstream::new(nats.clone());

    provision_stream(&js).await.expect("Failed to provision SESSION_DREAMS stream");
    let kv = provision_kv(&js).await.expect("Failed to provision SESSION_MEMORIES bucket");

    let provider = AnthropicMemoryProvider::new(config.llm);
    let dreamer = Dreamer::new(provider, kv.clone());
    let dreaming_service = DreamingService::new(js, config.consumer_name, dreamer);
    let writer = MemoryWriter::new(nats, kv.clone());

    tokio::join!(
        async move { dreaming_service.run().await.ok(); },
        async move { writer.run().await.ok(); },
        async move { serve(config.port, kv).await.expect("Memory management API failed"); },
    );
}
