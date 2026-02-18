//! Discord Bot Bridge for TrogonAI
//!
//! Acts as a bridge between Discord and NATS, converting Discord events into
//! NATS events and processing NATS commands to interact with Discord.

mod bridge;
mod config;
mod errors;
mod handlers;
mod health;
mod outbound;
mod outbound_streaming;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use discord_nats::connect;
use serenity::model::gateway::GatewayIntents;
use serenity::prelude::*;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::bridge::DiscordBridge;
use crate::config::Config;
use crate::handlers::Handler;
use crate::health::AppState;
use crate::outbound::OutboundProcessor;

/// Discord Bot Bridge CLI
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config/discord-bot.toml")]
    config: String,

    /// NATS URL (overrides config file)
    #[arg(long, env = "NATS_URL")]
    nats_url: Option<String>,

    /// Discord bot token (overrides config file)
    #[arg(long, env = "DISCORD_BOT_TOKEN")]
    bot_token: Option<String>,

    /// NATS prefix (overrides config file)
    #[arg(long, env = "DISCORD_PREFIX")]
    prefix: Option<String>,

    /// Health check server port
    #[arg(long, env = "HEALTH_CHECK_PORT", default_value = "3001")]
    health_port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "discord_bot=debug,discord_nats=debug,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Discord Bot Bridge");

    // Parse CLI arguments
    let args = Args::parse();

    // Load configuration
    let config = if std::path::Path::new(&args.config).exists() {
        info!("Loading config from file: {}", args.config);
        let mut config = Config::from_file(&args.config)?;

        if let Some(nats_url) = args.nats_url {
            config.nats.servers = nats_url.split(',').map(|s| s.to_string()).collect();
        }
        if let Some(bot_token) = args.bot_token {
            config.discord.bot_token = bot_token;
        }
        if let Some(prefix) = args.prefix {
            config.nats.prefix = prefix;
        }

        config
    } else {
        info!("Config file not found, loading from environment");
        let mut config = Config::from_env()?;

        if let Some(nats_url) = args.nats_url {
            config.nats.servers = nats_url.split(',').map(|s| s.to_string()).collect();
        }
        if let Some(bot_token) = args.bot_token {
            config.discord.bot_token = bot_token;
        }
        if let Some(prefix) = args.prefix {
            config.nats.prefix = prefix;
        }

        config
    };

    info!("NATS prefix: {}", config.nats.prefix);

    // Warn about suspicious access-control configuration
    for w in config.discord.access.warnings() {
        warn!("Access config: {}", w);
    }

    // Connect to NATS
    let nats_client = connect(&config.nats).await?;
    info!("Connected to NATS");

    // Build the Discord → NATS bridge
    let bridge = Arc::new(DiscordBridge::new(
        nats_client.clone(),
        config.nats.prefix.clone(),
        config.discord.access.clone(),
        config.discord.presence_enabled,
        config.discord.guild_commands_guild_id,
    ));

    // Extract pairing_state before bridge is moved into TypeMap so it can be
    // shared with the outbound processor.
    let pairing_state = bridge.pairing_state.clone();

    // Build the outbound processor (NATS → Discord)
    // We need the HTTP client from serenity for this, so we set it up after the client is built.
    let prefix = config.nats.prefix.clone();
    let bot_token = config.discord.bot_token.clone();
    let health_port = args.health_port;

    // Build serenity client
    let mut intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_MESSAGE_REACTIONS
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILDS
        | GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::GUILD_MESSAGE_TYPING
        | GatewayIntents::DIRECT_MESSAGE_TYPING;
    if config.discord.presence_enabled {
        intents |= GatewayIntents::GUILD_PRESENCES;
    }

    let mut client = Client::builder(&bot_token, intents)
        .event_handler(Handler)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create Discord client: {}", e))?;

    // Set up health check state before inserting into client data
    let health_state = AppState::new();

    // Insert bridge and health state into client data
    {
        let mut data = client.data.write().await;
        data.insert::<DiscordBridge>(bridge);
        data.insert::<AppState>(health_state.clone());
    }

    // Start health check server
    let health_state_clone = health_state.clone();
    tokio::spawn(async move {
        if let Err(e) = health::start_health_server(health_state_clone, health_port).await {
            error!("Health server error: {}", e);
        }
    });

    // Start outbound processor using the HTTP client from serenity
    let http = client.http.clone();
    let nats_client_for_outbound = nats_client.clone();
    let prefix_for_outbound = prefix.clone();
    let shard_manager_for_outbound = client.shard_manager.clone();

    tokio::spawn(async move {
        let processor = OutboundProcessor::new(
            http,
            nats_client_for_outbound,
            prefix_for_outbound,
            Some(shard_manager_for_outbound),
            pairing_state,
        );
        if let Err(e) = processor.run().await {
            error!("Outbound processor error: {}", e);
        }
    });

    // Graceful shutdown: close all shards on SIGTERM or Ctrl+C.
    let shard_manager = client.shard_manager.clone();
    tokio::spawn(async move {
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
        info!("Shutdown signal received, stopping Discord client...");
        shard_manager.shutdown_all().await;
    });

    info!("Starting Discord gateway connection...");

    // Start the Discord client (blocks until all shards are stopped)
    client
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("Discord client error: {}", e))?;

    info!("Discord bot stopped");
    Ok(())
}
