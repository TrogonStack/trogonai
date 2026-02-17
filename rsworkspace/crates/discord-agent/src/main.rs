//! Discord Agent - AI agent that processes Discord messages
//!
//! Listens to Discord events from NATS and responds with AI-generated messages.

mod agent;
mod conversation;
mod llm;
mod processor;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::agent::DiscordAgent;

/// Discord Agent CLI
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// NATS prefix
    #[arg(long, env = "DISCORD_PREFIX", default_value = "prod")]
    prefix: String,

    /// Agent name (for logging and identification)
    #[arg(long, env = "AGENT_NAME", default_value = "discord-agent")]
    agent_name: String,

    /// Claude API key
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    anthropic_api_key: Option<String>,

    /// Claude model to use
    #[arg(long, env = "CLAUDE_MODEL", default_value = "claude-sonnet-4-6")]
    claude_model: String,

    /// Enable LLM mode (requires API key)
    #[arg(long, env = "ENABLE_LLM", default_value = "false")]
    enable_llm: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "discord_agent=debug,discord_nats=debug,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Discord Agent");

    let args = Args::parse();

    info!("Agent name: {}", args.agent_name);
    info!("NATS URL: {}", args.nats_url);
    info!("NATS prefix: {}", args.prefix);

    // Connect to NATS
    let nats_config =
        discord_nats::NatsConfig::from_url(&args.nats_url, args.prefix.clone());
    let nats_client = discord_nats::connect(&nats_config).await?;
    info!("Connected to NATS successfully");

    // Setup JetStream KV for persistent conversation history
    let conversation_kv = {
        let js = async_nats::jetstream::new(nats_client.clone());
        let bucket = format!("discord_conversations_{}", args.prefix);
        match js.get_key_value(&bucket).await {
            Ok(kv) => {
                info!("Using existing conversation KV bucket: {}", bucket);
                Some(kv)
            }
            Err(_) => {
                match js
                    .create_key_value(async_nats::jetstream::kv::Config {
                        bucket: bucket.clone(),
                        history: 1,
                        storage: async_nats::jetstream::stream::StorageType::File,
                        ..Default::default()
                    })
                    .await
                {
                    Ok(kv) => {
                        info!("Created conversation KV bucket: {}", bucket);
                        Some(kv)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Could not create conversation KV bucket (running without persistence): {}",
                            e
                        );
                        None
                    }
                }
            }
        }
    };

    // Configure LLM if enabled
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

    let agent = DiscordAgent::new(
        nats_client,
        args.prefix,
        args.agent_name,
        llm_config,
        conversation_kv,
    );

    info!("Agent initialized, starting message processing...");

    if let Err(e) = agent.run().await {
        error!("Agent error: {}", e);
        return Err(e);
    }

    info!("Discord agent stopped");
    Ok(())
}
