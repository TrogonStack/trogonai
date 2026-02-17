//! Telegram Bot Bridge for TrogonAi
//!
//! This bot acts as a bridge between Telegram and NATS, converting
//! Telegram updates into NATS events and processing NATS commands
//! to send messages back to Telegram.

mod bridge;
mod config;
mod errors;
mod handlers;
mod health;
mod outbound;
mod outbound_streaming;
mod session;

use anyhow::Result;
use clap::Parser;
use teloxide::prelude::*;
use teloxide::types::Message;
use tracing::{info, error};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::bridge::TelegramBridge;
use crate::config::Config;
use crate::outbound::OutboundProcessor;

/// Telegram Bot Bridge CLI
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config/telegram-bot.toml")]
    config: String,

    /// NATS URL (overrides config file)
    #[arg(long, env = "NATS_URL")]
    nats_url: Option<String>,

    /// Telegram bot token (overrides config file)
    #[arg(long, env = "TELEGRAM_BOT_TOKEN")]
    bot_token: Option<String>,

    /// NATS prefix (overrides config file)
    #[arg(long, env = "TELEGRAM_PREFIX")]
    prefix: Option<String>,

    /// Health check server port
    #[arg(long, env = "HEALTH_CHECK_PORT", default_value = "3000")]
    health_port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "telegram_bot=debug,telegram_nats=debug,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Telegram Bot Bridge");

    // Parse CLI arguments
    let args = Args::parse();

    // Load configuration
    let config = if std::path::Path::new(&args.config).exists() {
        info!("Loading config from file: {}", args.config);
        let mut config = Config::from_file(&args.config)?;

        // Override with CLI arguments
        if let Some(nats_url) = args.nats_url {
            config.nats.servers = nats_url.split(',').map(|s| s.to_string()).collect();
        }
        if let Some(bot_token) = args.bot_token {
            config.telegram.bot_token = bot_token;
        }
        if let Some(prefix) = args.prefix {
            config.nats.prefix = prefix;
        }

        config
    } else {
        info!("Config file not found, using environment variables");
        Config::from_env()?
    };

    info!("Configuration loaded successfully");
    info!("NATS servers: {:?}", config.nats.servers);
    info!("NATS prefix: {}", config.nats.prefix);

    // Connect to NATS
    info!("Connecting to NATS...");
    let nats_client = telegram_nats::connect(&config.nats).await?;
    info!("Connected to NATS successfully");

    // Setup JetStream
    let js = telegram_nats::nats::jetstream(&nats_client).await;
    telegram_nats::nats::setup_event_stream(&js, &config.nats.prefix).await?;
    let kv = telegram_nats::nats::setup_session_kv(&js, &config.nats.prefix).await?;
    info!("JetStream setup complete");

    // Create Telegram bot
    info!("Initializing Telegram bot...");
    let bot = Bot::new(&config.telegram.bot_token);

    // Verify bot token
    let bot_username = match bot.get_me().await {
        Ok(me) => {
            let username = me.username().to_string();
            info!("Bot authenticated as: @{}", username);
            Some(username)
        }
        Err(e) => {
            error!("Failed to authenticate bot: {}", e);
            return Err(e.into());
        }
    };

    // Create health check state
    let health_state = health::AppState::new(bot_username);

    // Mark NATS as connected
    *health_state.nats_connected.write().await = true;

    // Start health check server
    let health_state_clone = health_state.clone();
    let health_port = args.health_port;
    tokio::spawn(async move {
        if let Err(e) = health::start_health_server(health_state_clone, health_port).await {
            error!("Health check server error: {}", e);
        }
    });

    // Create bridge
    let bridge = TelegramBridge::new(
        nats_client.clone(),
        config.nats.prefix.clone(),
        config.telegram.access.clone(),
        kv,
    );

    // Start outbound processor (NATS → Telegram)
    let outbound = OutboundProcessor::new(
        bot.clone(),
        nats_client.clone(),
        config.nats.prefix.clone(),
    );

    tokio::spawn(async move {
        if let Err(e) = outbound.run().await {
            error!("Outbound processor error: {}", e);
        }
    });

    info!("Bot initialized, starting message dispatcher...");

    // Setup dispatcher with proper handler tree
    let handler = Update::filter_message()
        .branch(
            dptree::filter(|msg: Message| msg.text().is_some())
                .endpoint(handlers::handle_text_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.photo().is_some())
                .endpoint(handlers::handle_photo_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.video().is_some())
                .endpoint(handlers::handle_video_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.audio().is_some())
                .endpoint(handlers::handle_audio_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.document().is_some())
                .endpoint(handlers::handle_document_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.voice().is_some())
                .endpoint(handlers::handle_voice_message),
        );

    let callback_handler = Update::filter_callback_query()
        .endpoint(handlers::handle_callback_query);

    let inline_query_handler = Update::filter_inline_query()
        .endpoint(handlers::handle_inline_query);

    let chosen_inline_result_handler = Update::filter_chosen_inline_result()
        .endpoint(handlers::handle_chosen_inline_result);

    let chat_member_handler = Update::filter_chat_member()
        .endpoint(handlers::handle_chat_member_updated);

    let my_chat_member_handler = Update::filter_my_chat_member()
        .endpoint(handlers::handle_my_chat_member_updated);

    let pre_checkout_handler = Update::filter_pre_checkout_query()
        .endpoint(handlers::handle_pre_checkout_query);

    let shipping_query_handler = Update::filter_shipping_query()
        .endpoint(handlers::handle_shipping_query);

    // Successful payments arrive as messages with successful_payment field
    let successful_payment_handler = Update::filter_message()
        .filter(|msg: Message| msg.successful_payment().is_some())
        .endpoint(handlers::handle_successful_payment);

    let all_handlers = dptree::entry()
        .branch(handler)
        .branch(callback_handler)
        .branch(inline_query_handler)
        .branch(chosen_inline_result_handler)
        .branch(chat_member_handler)
        .branch(my_chat_member_handler)
        .branch(pre_checkout_handler)
        .branch(shipping_query_handler)
        .branch(successful_payment_handler);

    // Start bot based on update mode
    match &config.telegram.update_mode {
        crate::config::UpdateModeConfig::Polling { timeout, limit } => {
            info!("Starting bot in POLLING mode (timeout: {}s, limit: {})", timeout, limit);
        }
        crate::config::UpdateModeConfig::Webhook {
            url,
            port,
            path,
            secret_token,
            bind_address,
            max_connections,
        } => {
            info!("Starting bot in WEBHOOK mode");
            info!("Webhook URL: {}{}", url, path);
            info!("Listening on: {}:{}", bind_address, port);

            // Parse webhook URL
            let webhook_url = format!("{}{}", url, path);
            let parsed_url = match webhook_url.parse::<url::Url>() {
                Ok(u) => u,
                Err(e) => {
                    error!("Invalid webhook URL '{}': {}", webhook_url, e);
                    return Err(e.into());
                }
            };

            // Set webhook on Telegram
            let mut set_webhook = bot.set_webhook(parsed_url);

            if let Some(token) = secret_token {
                set_webhook.secret_token = Some(token.clone());
            }

            if *max_connections > 0 {
                set_webhook.max_connections = Some(*max_connections);
            }

            if let Err(e) = set_webhook.await {
                error!("Failed to set webhook: {}", e);
                return Err(e.into());
            }

            info!("Webhook registered with Telegram successfully");
            info!("NOTE: Webhook mode configured - using polling until webhook listener is fully implemented");
            info!("The bot will receive updates via polling, but webhook is set on Telegram's side");
        }
    }

    // Start dispatcher (currently using polling for both modes)
    // TODO: Implement proper webhook listener for webhook mode
    Dispatcher::builder(bot, all_handlers)
        .dependencies(dptree::deps![bridge, health_state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    info!("Telegram bot stopped");
    Ok(())
}
