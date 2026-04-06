mod agent;
mod conversation;
mod llm;
mod processor;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info};

use crate::agent::TelegramAgent;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, env = "TELEGRAM_PREFIX", default_value = "prod")]
    prefix: String,

    #[arg(long, env = "AGENT_NAME", default_value = "telegram-agent")]
    agent_name: String,

    #[arg(long, env = "ANTHROPIC_API_KEY")]
    anthropic_api_key: Option<String>,

    #[arg(
        long,
        env = "CLAUDE_MODEL",
        default_value = "claude-sonnet-4-5-20250929"
    )]
    claude_model: String,

    #[arg(long, env = "ENABLE_LLM", default_value = "false")]
    enable_llm: bool,
}

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<()> {
    use trogon_std::env::SystemEnv;
    use trogon_std::fs::SystemFs;

    let telegram_config = telegram_nats::TelegramNatsConfig::from_env(&SystemEnv);

    acp_telemetry::init_logger(
        acp_telemetry::ServiceName::TelegramAgent,
        &telegram_config.prefix,
        &SystemEnv,
        &SystemFs,
    );

    info!("Starting Telegram Agent");

    let args = Args::parse();

    info!("Agent name: {}", args.agent_name);
    info!("NATS servers: {:?}", telegram_config.nats.servers);
    info!("NATS prefix: {}", args.prefix);

    info!("Connecting to NATS...");
    let nats_client = trogon_nats::connect(
        &telegram_config.nats,
        std::time::Duration::from_secs(10),
    )
    .await?;
    info!("Connected to NATS successfully");

    let js = async_nats::jetstream::new(nats_client.clone());

    telegram_nats::nats::setup_event_stream(&js, &args.prefix).await?;
    telegram_nats::nats::setup_agent_stream(&js, &args.prefix).await?;

    async fn get_or_create_kv(
        js: &async_nats::jetstream::Context,
        bucket: &str,
        ttl_secs: Option<u64>,
    ) -> Option<async_nats::jetstream::kv::Store> {
        if let Ok(kv) = js.get_key_value(bucket).await {
            info!("Using existing KV bucket: {}", bucket);
            return Some(kv);
        }
        let max_age = ttl_secs
            .map(std::time::Duration::from_secs)
            .unwrap_or_default();
        match js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: bucket.to_string(),
                history: 1,
                max_age,
                storage: async_nats::jetstream::stream::StorageType::File,
                ..Default::default()
            })
            .await
        {
            Ok(kv) => {
                info!("Created KV bucket: {}", bucket);
                Some(kv)
            }
            Err(e) => {
                tracing::warn!("Could not create KV bucket '{}': {}", bucket, e);
                None
            }
        }
    }

    let js_kv = async_nats::jetstream::new(nats_client.clone());

    let conversation_kv = get_or_create_kv(
        &js_kv,
        &format!("telegram_conversations_{}", args.prefix),
        None,
    )
    .await;

    let dedup_kv = get_or_create_kv(
        &js_kv,
        &format!("telegram_dedup_{}", args.prefix),
        Some(86_400),
    )
    .await;

    let llm_config = if args.enable_llm {
        if let Some(api_key) = args.anthropic_api_key {
            info!("LLM mode enabled with model: {}", args.claude_model);
            Some(llm::ClaudeConfig {
                api_key,
                model: args.claude_model,
                max_tokens: 1024,
                temperature: 1.0,
            })
        } else {
            error!("LLM mode enabled but ANTHROPIC_API_KEY not provided");
            return Err(anyhow::anyhow!(
                "ANTHROPIC_API_KEY required when --enable-llm is set"
            ));
        }
    } else {
        info!("LLM mode disabled, running in echo mode");
        None
    };

    let agent = TelegramAgent::new(
        nats_client,
        js,
        args.prefix,
        args.agent_name,
        llm_config,
        conversation_kv,
        dedup_kv,
    );

    info!("Agent initialized, starting message processing...");

    if let Err(e) = agent.run().await {
        error!("Agent error: {}", e);
        return Err(e);
    }

    info!("Telegram agent stopped");
    acp_telemetry::shutdown_otel();
    Ok(())
}

#[cfg(coverage)]
fn main() {}

#[cfg(all(coverage, test))]
mod tests {
    #[test]
    fn coverage_stub() {
        super::main();
    }
}
