use std::error::Error;
use std::time::Duration;

use tracing::info;
use tracing_subscriber::EnvFilter;
use trogon_nats::{NatsAuth, NatsConfig, connect};
use trogon_std::signal::shutdown_signal;

use trogon_agent_registry::{AgentRegistryStore, RegistryCache, open_bucket, run_lookup_consumer, spawn_watch_task};

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

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let auto_create = env_var_is_truthy("TROGON_REGISTRY_AUTOCREATE");

    info!(nats_url = %nats_url, auto_create, "starting trogon-agent-registry");

    let nats_config = NatsConfig::new(vec![nats_url], NatsAuth::None);
    let client = connect(&nats_config, Duration::from_secs(15)).await?;
    let jetstream = async_nats::jetstream::new(client.clone());

    let kv = open_bucket(&jetstream, auto_create).await?;
    let store = AgentRegistryStore::new(kv, client.clone());
    let cache = RegistryCache::new();

    store.warm_cache(cache.clone()).await?;
    spawn_watch_task(store.clone(), cache.clone()).await?;

    info!("registry KV ready; lookup consumer listening");
    run_lookup_consumer(client, store, cache, shutdown_signal()).await?;

    info!("trogon-agent-registry stopped");
    Ok(())
}
