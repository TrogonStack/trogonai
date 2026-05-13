use std::time::Duration;

use trogon_nats::connect;
use trogon_registry::{Registry, provision};
use trogon_std::env::SystemEnv;
use trogon_orchestrator::{
    AnthropicOrchestratorProvider, OrchestratorConfig, OrchestratorEngine,
    caller::NatsAgentCaller,
    serve,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = OrchestratorConfig::from_env(&SystemEnv);
    let nats = connect(&config.nats, Duration::from_secs(10))
        .await
        .expect("Failed to connect to NATS");

    let js = async_nats::jetstream::new(nats.clone());

    let registry_store = provision(&js).await.expect("Failed to provision registry");
    let registry = Registry::new(registry_store);

    let caller = NatsAgentCaller::new(nats)
        .with_timeout(Duration::from_secs(config.agent_timeout_secs));

    let provider = AnthropicOrchestratorProvider::new(config.llm);
    let engine = OrchestratorEngine::new(provider, caller, registry);

    serve(config.port, engine).await.expect("orchestrator server failed");
}
