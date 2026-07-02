mod bridge;
mod config;
mod dedup;
mod errors;
mod handlers;
mod health;
mod outbound;
mod outbound_streaming;
mod session;

use anyhow::Result;
use clap::Parser;
use futures::Stream;
use futures::StreamExt;
use teloxide::prelude::*;
use teloxide::types::Message;
use tracing::{error, info, warn};

use crate::bridge::TelegramBridge;
use crate::config::Config;
use crate::outbound::OutboundProcessor;

struct GatewayListenerState {
    messages: async_nats::jetstream::consumer::pull::Stream,
    stop_token: teloxide::stop::StopToken,
}

fn transform_gateway_stream(
    state: &mut GatewayListenerState,
) -> impl Stream<Item = Result<teloxide::types::Update, std::convert::Infallible>> + Send + '_ {
    futures::stream::unfold(&mut state.messages, |messages| async move {
        loop {
            let msg = match messages.next().await {
                Some(Ok(msg)) => msg,
                Some(Err(e)) => {
                    tracing::warn!("Error receiving message from gateway stream: {}", e);
                    continue;
                }
                None => return None,
            };
            let body_len = msg.payload.len();
            if let Err(e) = msg.ack().await {
                tracing::warn!("Failed to ack gateway message: {}", e);
            }
            match serde_json::from_slice::<teloxide::types::Update>(&msg.payload) {
                Ok(update) => return Some((Ok(update), messages)),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse Telegram Update from gateway message (body_len={}): {}",
                        body_len,
                        e
                    );
                    continue;
                }
            }
        }
    })
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "config/telegram-bot.toml")]
    config: String,

    #[arg(long, env = "TELEGRAM_BOT_TOKEN")]
    bot_token: Option<String>,

    #[arg(long, env = "TELEGRAM_PREFIX")]
    prefix: Option<String>,

    #[arg(long, env = "HEALTH_CHECK_PORT", default_value = "3000")]
    health_port: u16,
}

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<()> {
    use trogon_std::env::SystemEnv;
    use trogon_std::fs::SystemFs;

    let telegram_config = telegram_nats::TelegramNatsConfig::from_env(&SystemEnv);

    trogon_telemetry::init_logger(
        trogon_telemetry::ServiceName::TelegramBot,
        [],
        &SystemEnv,
        &SystemFs,
    );

    info!("Starting Telegram Bot Bridge");

    let args = Args::parse();

    let config = if std::path::Path::new(&args.config).exists() {
        info!("Loading config from file: {}", args.config);
        let mut config = Config::from_file(&args.config)?;

        if let Some(bot_token) = args.bot_token {
            config.telegram.bot_token = bot_token;
        }
        if let Some(prefix) = args.prefix {
            config.prefix = prefix;
        }

        config
    } else {
        info!("Config file not found, using environment variables");
        Config::from_env(&SystemEnv)?
    };

    info!("NATS servers: {:?}", telegram_config.nats.servers);
    info!("NATS prefix: {}", config.prefix);

    info!("Connecting to NATS...");
    let nats_client = trogon_nats::connect(
        &telegram_config.nats,
        std::time::Duration::from_secs(10),
    )
    .await?;
    info!("Connected to NATS successfully");

    let js = async_nats::jetstream::new(nats_client.clone());
    telegram_nats::nats::setup_event_stream(&js, &config.prefix).await?;
    telegram_nats::nats::setup_agent_stream(&js, &config.prefix).await?;
    let kv = telegram_nats::nats::setup_session_kv(&js, &config.prefix).await?;
    let dedup_kv = telegram_nats::nats::setup_dedup_kv(&js, &config.prefix).await?;
    info!("JetStream setup complete");

    let inbound_stream = js.get_stream(&config.inbound_stream_name).await.map_err(|e| {
        anyhow::anyhow!(
            "Gateway inbound stream '{}' not found; the trogon-gateway Telegram source must provision it: {e}",
            config.inbound_stream_name
        )
    })?;

    let consumer_name = format!("telegram-bot-{}", config.prefix);
    let consumer = inbound_stream
        .get_or_create_consumer(
            &consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
                ack_wait: std::time::Duration::from_secs(30),
                max_deliver: 5,
                ..Default::default()
            },
        )
        .await?;
    let messages = consumer.messages().await?;

    info!("Initializing Telegram bot...");
    let bot = Bot::new(&config.telegram.bot_token);

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

    let health_state = health::AppState::new(bot_username);
    *health_state.nats_connected.write().await = true;

    let health_state_clone = health_state.clone();
    let health_port = args.health_port;
    tokio::spawn(async move {
        if let Err(e) = health::start_health_server(health_state_clone, health_port).await {
            warn!(
                "Health check server failed to start on port {} (non-fatal): {}",
                health_port, e
            );
        }
    });

    let bridge = TelegramBridge::new(
        nats_client.clone(),
        config.prefix.clone(),
        config.telegram.access.clone(),
        kv,
        crate::dedup::DedupStore::new(dedup_kv),
    );

    let cmd_dedup_kv = {
        let bucket = format!("telegram_cmd_dedup_{}", config.prefix);
        match js.get_key_value(&bucket).await {
            Ok(kv) => {
                info!("Using existing cmd dedup bucket: {}", bucket);
                Some(kv)
            }
            Err(_) => match js
                .create_key_value(async_nats::jetstream::kv::Config {
                    bucket: bucket.clone(),
                    history: 1,
                    max_age: std::time::Duration::from_secs(3_600),
                    storage: async_nats::jetstream::stream::StorageType::Memory,
                    ..Default::default()
                })
                .await
            {
                Ok(kv) => {
                    info!("Created cmd dedup bucket: {}", bucket);
                    Some(kv)
                }
                Err(e) => {
                    warn!("Could not create cmd dedup bucket '{}': {}", bucket, e);
                    None
                }
            },
        }
    };

    let outbound = OutboundProcessor::new(
        bot.clone(),
        nats_client.clone(),
        config.prefix.clone(),
        js,
        cmd_dedup_kv,
    );

    tokio::spawn(async move {
        if let Err(e) = outbound.run().await {
            error!("Outbound processor error: {}", e);
        }
    });

    info!("Bot initialized, starting message dispatcher...");

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
        )
        .branch(
            dptree::filter(|msg: Message| msg.location().is_some())
                .endpoint(handlers::handle_location_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.venue().is_some())
                .endpoint(handlers::handle_venue_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.contact().is_some())
                .endpoint(handlers::handle_contact_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.sticker().is_some())
                .endpoint(handlers::handle_sticker_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.animation().is_some())
                .endpoint(handlers::handle_animation_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.video_note().is_some())
                .endpoint(handlers::handle_video_note_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.poll().is_some())
                .endpoint(handlers::handle_poll_message),
        )
        .branch(
            dptree::filter(|msg: Message| msg.forum_topic_created().is_some())
                .endpoint(handlers::handle_forum_topic_created),
        )
        .branch(
            dptree::filter(|msg: Message| msg.forum_topic_edited().is_some())
                .endpoint(handlers::handle_forum_topic_edited),
        )
        .branch(
            dptree::filter(|msg: Message| msg.forum_topic_closed().is_some())
                .endpoint(handlers::handle_forum_topic_closed),
        )
        .branch(
            dptree::filter(|msg: Message| msg.forum_topic_reopened().is_some())
                .endpoint(handlers::handle_forum_topic_reopened),
        )
        .branch(
            dptree::filter(|msg: Message| msg.general_forum_topic_hidden().is_some())
                .endpoint(handlers::handle_general_forum_topic_hidden),
        )
        .branch(
            dptree::filter(|msg: Message| msg.general_forum_topic_unhidden().is_some())
                .endpoint(handlers::handle_general_forum_topic_unhidden),
        );

    let edited_message_handler =
        Update::filter_edited_message().endpoint(handlers::handle_edited_message);

    let channel_post_handler =
        Update::filter_channel_post().endpoint(handlers::handle_channel_post);

    let edited_channel_post_handler =
        Update::filter_edited_channel_post().endpoint(handlers::handle_edited_channel_post);

    let chat_join_request_handler =
        Update::filter_chat_join_request().endpoint(handlers::handle_chat_join_request);

    let poll_update_handler = Update::filter_poll().endpoint(handlers::handle_poll_update);

    let poll_answer_handler = Update::filter_poll_answer().endpoint(handlers::handle_poll_answer);

    let callback_handler =
        Update::filter_callback_query().endpoint(handlers::handle_callback_query);

    let inline_query_handler =
        Update::filter_inline_query().endpoint(handlers::handle_inline_query);

    let chosen_inline_result_handler =
        Update::filter_chosen_inline_result().endpoint(handlers::handle_chosen_inline_result);

    let chat_member_handler =
        Update::filter_chat_member().endpoint(handlers::handle_chat_member_updated);

    let my_chat_member_handler =
        Update::filter_my_chat_member().endpoint(handlers::handle_my_chat_member_updated);

    let pre_checkout_handler =
        Update::filter_pre_checkout_query().endpoint(handlers::handle_pre_checkout_query);

    let shipping_query_handler =
        Update::filter_shipping_query().endpoint(handlers::handle_shipping_query);

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
        .branch(successful_payment_handler)
        .branch(poll_update_handler)
        .branch(poll_answer_handler)
        .branch(edited_message_handler)
        .branch(channel_post_handler)
        .branch(edited_channel_post_handler)
        .branch(chat_join_request_handler);

    let mut dispatcher = Dispatcher::builder(bot.clone(), all_handlers)
        .dependencies(dptree::deps![bridge, health_state])
        .enable_ctrlc_handler()
        .build();

    info!(
        "Consuming Telegram updates from gateway stream '{}' via consumer '{}'",
        config.inbound_stream_name, consumer_name
    );

    let (stop_token, _stop_flag) = teloxide::stop::mk_stop_token();
    let listener_state = GatewayListenerState {
        messages,
        stop_token,
    };
    let listener = teloxide::update_listeners::StatefulListener::new(
        listener_state,
        transform_gateway_stream,
        |state: &mut GatewayListenerState| state.stop_token.clone(),
    );

    dispatcher
        .dispatch_with_listener(
            listener,
            std::sync::Arc::new(teloxide::error_handlers::IgnoringErrorHandlerSafe),
        )
        .await;

    info!("Telegram bot stopped");
    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }
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
