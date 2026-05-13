use std::time::Duration;

use trogon_nats::connect;
use trogon_std::env::SystemEnv;
use trogon_webhook_dispatcher::{
    Dispatcher, ReqwestWebhookClient, WebhookDispatcherConfig, WebhookRegistry, provision, serve,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = WebhookDispatcherConfig::from_env(&SystemEnv);
    let nats = connect(&config.nats, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let js = async_nats::jetstream::new(nats.clone());
    let kv = provision(&js).await.expect("Failed to provision KV bucket");

    let registry = WebhookRegistry::new(kv);
    let http = ReqwestWebhookClient::new(config.dispatch_timeout);

    let dispatcher = Dispatcher::new(
        js,
        config.stream_name.clone(),
        config.consumer_name.clone(),
        config.subject_filter.clone(),
        registry.clone(),
        http,
    );

    tokio::join!(
        async move { serve(config.port, registry).await.expect("management API failed") },
        async move { dispatcher.run().await.ok(); },
    );
}
