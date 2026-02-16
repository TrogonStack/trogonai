//! Telegram Agent - AI agent that processes Telegram messages
//!
//! This agent listens to Telegram events from NATS and responds with
//! AI-generated messages.

mod agent;
mod processor;

use anyhow::Result;
use clap::Parser;
use tracing::{info, error};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::agent::TelegramAgent;

/// Telegram Agent CLI
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// NATS URL
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats_url: String,

    /// NATS prefix
    #[arg(long, env = "TELEGRAM_PREFIX", default_value = "prod")]
    prefix: String,

    /// Agent name (for logging and identification)
    #[arg(long, env = "AGENT_NAME", default_value = "telegram-agent")]
    agent_name: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "telegram_agent=debug,telegram_nats=debug,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Telegram Agent");

    // Parse CLI arguments
    let args = Args::parse();

    info!("Agent name: {}", args.agent_name);
    info!("NATS URL: {}", args.nats_url);
    info!("NATS prefix: {}", args.prefix);

    // Connect to NATS
    info!("Connecting to NATS...");
    let servers: Vec<String> = args.nats_url.split(',').map(|s| s.to_string()).collect();
    let nats_config = telegram_nats::NatsConfig {
        servers,
        prefix: args.prefix.clone(),
        credentials_file: None,
        username: None,
        password: None,
    };

    let nats_client = telegram_nats::connect(&nats_config).await?;
    info!("Connected to NATS successfully");

    // Create and run agent
    let agent = TelegramAgent::new(
        nats_client,
        args.prefix,
        args.agent_name,
    );

    info!("Agent initialized, starting message processing...");

    // Run agent
    if let Err(e) = agent.run().await {
        error!("Agent error: {}", e);
        return Err(e);
    }

    info!("Telegram agent stopped");
    Ok(())
}
