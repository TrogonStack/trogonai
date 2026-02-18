//! Discord Agent - AI agent that processes Discord messages
//!
//! Listens to Discord events from NATS and responds with AI-generated messages.

mod agent;
mod conversation;
mod conversation_tests;
mod health;
mod llm;
mod processor;
mod processor_tests;

use anyhow::Result;
use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::agent::DiscordAgent;
use crate::health::AgentHealthState;
use crate::processor::WelcomeConfig;

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

    /// System prompt for the LLM (uses built-in default if not set)
    #[arg(long, env = "SYSTEM_PROMPT")]
    system_prompt: Option<String>,

    /// Maximum tokens per LLM response
    #[arg(long, env = "CLAUDE_MAX_TOKENS", default_value = "1024")]
    max_tokens: u32,

    /// LLM sampling temperature (0.0–1.0)
    #[arg(long, env = "CLAUDE_TEMPERATURE", default_value = "1.0")]
    temperature: f32,

    /// Health check server port (0 to disable)
    #[arg(long, env = "HEALTH_CHECK_PORT", default_value = "3002")]
    health_port: u16,

    /// Conversation history TTL in hours (0 = never expire, default 168 = 7 days)
    #[arg(long, env = "CONVERSATION_TTL_HOURS", default_value = "168")]
    conversation_ttl_hours: u64,

    /// Channel ID for welcome messages (0 to disable)
    #[arg(long, env = "WELCOME_CHANNEL_ID", default_value = "0")]
    welcome_channel_id: u64,

    /// Welcome message template ({user} = mention, {username} = display name)
    #[arg(long, env = "WELCOME_MESSAGE")]
    welcome_message: Option<String>,

    /// Channel ID for farewell messages (0 to disable)
    #[arg(long, env = "FAREWELL_CHANNEL_ID", default_value = "0")]
    farewell_channel_id: u64,

    /// Farewell message template ({user} = mention, {username} = display name)
    #[arg(long, env = "FAREWELL_MESSAGE")]
    farewell_message: Option<String>,
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
    let nats_config = discord_nats::NatsConfig::from_url(&args.nats_url, args.prefix.clone());
    let nats_client = discord_nats::connect(&nats_config).await?;
    info!("Connected to NATS successfully");

    // Setup JetStream KV for persistent conversation history
    let ttl_duration = if args.conversation_ttl_hours > 0 {
        let d = std::time::Duration::from_secs(args.conversation_ttl_hours * 3600);
        info!(
            "Conversation TTL: {} hours ({} days)",
            args.conversation_ttl_hours,
            args.conversation_ttl_hours / 24
        );
        d
    } else {
        info!("Conversation TTL: disabled (sessions never expire)");
        std::time::Duration::ZERO
    };

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
                        max_age: ttl_duration,
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
                max_tokens: args.max_tokens,
                temperature: args.temperature,
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

    let mode = if llm_config.is_some() { "llm" } else { "echo" }.to_string();

    let welcome = if args.welcome_channel_id != 0 {
        let template = args
            .welcome_message
            .unwrap_or_else(|| processor::DEFAULT_WELCOME_TEMPLATE.to_string());
        info!(
            "Welcome messages enabled: channel={}, template={:?}",
            args.welcome_channel_id, template
        );
        Some(WelcomeConfig {
            channel_id: args.welcome_channel_id,
            template,
        })
    } else {
        None
    };

    let farewell = if args.farewell_channel_id != 0 {
        let template = args
            .farewell_message
            .unwrap_or_else(|| processor::DEFAULT_FAREWELL_TEMPLATE.to_string());
        info!(
            "Farewell messages enabled: channel={}, template={:?}",
            args.farewell_channel_id, template
        );
        Some(WelcomeConfig {
            channel_id: args.farewell_channel_id,
            template,
        })
    } else {
        None
    };

    let conversation_ttl = if args.conversation_ttl_hours > 0 {
        Some(tokio::time::Duration::from_secs(
            args.conversation_ttl_hours * 3600,
        ))
    } else {
        None
    };

    // Build health state first so we can share its metrics with the processor
    let health_state = AgentHealthState::new(args.agent_name.clone(), mode.clone());
    let metrics = Some(health_state.metrics.clone());

    let agent = DiscordAgent::new(
        nats_client,
        args.prefix,
        args.agent_name.clone(),
        llm_config,
        conversation_kv,
        args.system_prompt,
        welcome,
        farewell,
        conversation_ttl,
        metrics,
    );

    info!("Agent initialized, starting message processing...");

    // Start health server (unless disabled with port 0)
    if args.health_port != 0 {
        let port = args.health_port;
        tokio::spawn(async move {
            if let Err(e) = health::start_health_server(health_state, port).await {
                error!("Health server error: {}", e);
            }
        });
    }

    tokio::select! {
        result = agent.run() => {
            if let Err(e) = result {
                error!("Agent error: {}", e);
                return Err(e);
            }
        }
        _ = shutdown_signal() => {
            info!("Shutdown signal received, stopping agent...");
        }
    }

    info!("Discord agent stopped");
    Ok(())
}

/// Resolves on SIGTERM (Unix) or Ctrl+C.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
    }
}
