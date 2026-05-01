// TODO: remove once LlmAgent is wired up
#![allow(dead_code)]

mod api_key;
mod base_url;
mod config;
mod error;
mod model;
mod model_id;
mod provider;
mod provider_name;
mod session;
mod session_store;
mod telemetry;

use provider::LanguageModelProvider;

#[cfg(not(coverage))]
use {
    acp_nats::nats,
    acp_telemetry::ServiceName,
    tracing::{error, info},
    trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::build_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;

    acp_telemetry::init_logger(
        ServiceName::AcpAgentLlm,
        config.acp.acp_prefix(),
        &SystemEnv,
        &SystemFs,
    );

    info!("acp-agent-llm starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let _nats_client = nats::connect(config.acp.nats(), nats_connect_timeout).await?;

    // Build providers — only those with API keys.
    // Each provider fetches its available models from the API at startup.
    let mut providers: std::collections::HashMap<
        provider_name::ProviderName,
        Box<dyn provider::LanguageModelProvider>,
    > = std::collections::HashMap::new();

    if let Some(key) = config.anthropic_api_key {
        let mut p = provider::anthropic::AnthropicProvider::new(key, config.base_url.clone());
        p.fetch_models().await?;
        info!(
            models = p.provided_models().len(),
            "Anthropic models loaded"
        );
        providers.insert(
            provider_name::ProviderName::new("anthropic").expect("known"),
            Box::new(p),
        );
    }
    if let Some(key) = config.openai_api_key {
        let mut p = provider::openai::OpenAiProvider::new(key, config.base_url.clone());
        p.fetch_models().await?;
        info!(models = p.provided_models().len(), "OpenAI models loaded");
        providers.insert(
            provider_name::ProviderName::new("openai").expect("known"),
            Box::new(p),
        );
    }

    if providers.is_empty() {
        error!("No API keys configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
        return Err("no providers configured".into());
    }

    info!(
        provider = config.default_provider.as_str(),
        model = config.default_model.as_str(),
        "Default model configured"
    );

    // TODO: build LlmAgent and run on LocalSet with AgentSideNatsConnection
    // This will be wired up once the LlmAgent Agent trait impl is complete.
    info!("acp-agent-llm ready (agent loop not yet implemented)");

    // Wait for shutdown
    acp_telemetry::signal::shutdown_signal().await;
    info!("acp-agent-llm stopped");

    if let Err(e) = acp_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    Ok(())
}

#[cfg(coverage)]
fn main() {}
