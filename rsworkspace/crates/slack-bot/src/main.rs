mod config;
mod format;
mod health;
mod listener;
mod sender;

use async_nats::jetstream;
use config::SlackBotConfig;
use futures::StreamExt;
use listener::{
    BotState, error_handler, handle_command_event, handle_interaction_event, handle_push_event,
};
use sender::{
    run_outbound_loop, run_reaction_action_loop, run_stream_append_loop, run_stream_stop_loop,
};
use slack_morphism::prelude::*;
use slack_nats::setup::ensure_slack_stream;
use slack_nats::subscriber::{
    create_outbound_consumer, create_reaction_action_consumer, create_stream_append_consumer,
    create_stream_stop_consumer,
};
use slack_types::events::{SlackStreamStartRequest, SlackStreamStartResponse};
use slack_types::subjects::SLACK_OUTBOUND_STREAM_START;
use std::sync::Arc;
use std::time::Duration;
use trogon_nats::connect;
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let config = SlackBotConfig::from_env(&SystemEnv);

    tracing::info!(port = config.health_port, "Starting health check server...");
    tokio::spawn(health::start_health_server(config.health_port));

    tracing::info!("Connecting to NATS...");
    let nats_client = connect(&config.nats)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let nats_client = Arc::new(nats_client);

    tracing::info!("Setting up JetStream stream...");
    let js = Arc::new(jetstream::new((*nats_client).clone()));
    ensure_slack_stream(&js).await?;

    tracing::info!("Creating JetStream consumers...");
    let outbound_consumer = create_outbound_consumer(&js).await?;
    let stream_append_consumer = create_stream_append_consumer(&js).await?;
    let stream_stop_consumer = create_stream_stop_consumer(&js).await?;
    let reaction_action_consumer = create_reaction_action_consumer(&js).await?;

    // stream.start uses Core NATS request/reply — subscribe on the raw client.
    let mut stream_start_sub = nats_client.subscribe(SLACK_OUTBOUND_STREAM_START).await?;

    tracing::info!("Connecting to Slack via Socket Mode...");
    let slack_client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));

    let bot_state = BotState {
        nats: js.clone(),
        bot_user_id: config.bot_user_id.clone(),
        mention_gating: config.mention_gating,
        bot_token: config.bot_token.clone(),
        http_client: reqwest::Client::new(),
    };

    let socket_mode_callbacks = SlackSocketModeListenerCallbacks::new()
        .with_push_events(handle_push_event)
        .with_command_events(handle_command_event)
        .with_interaction_events(handle_interaction_event);

    let listener_environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(slack_client.clone())
            .with_error_handler(error_handler)
            .with_user_state(bot_state),
    );

    let socket_mode_listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_environment,
        socket_mode_callbacks,
    );

    let app_token: SlackApiToken = SlackApiToken::new(config.app_token.clone().into());
    socket_mode_listener.listen_for(&app_token).await?;

    tracing::info!("Slack bot running. Press Ctrl+C to stop.");

    let bot_token = config.bot_token.clone();

    let outbound_handle = tokio::spawn({
        let sc = slack_client.clone();
        let bt = bot_token.clone();
        async move { run_outbound_loop(outbound_consumer, sc, bt).await }
    });
    let outbound_abort = outbound_handle.abort_handle();

    let stream_append_handle = tokio::spawn({
        let sc = slack_client.clone();
        let bt = bot_token.clone();
        async move { run_stream_append_loop(stream_append_consumer, sc, bt).await }
    });
    let stream_append_abort = stream_append_handle.abort_handle();

    let stream_stop_handle = tokio::spawn({
        let sc = slack_client.clone();
        let bt = bot_token.clone();
        async move { run_stream_stop_loop(stream_stop_consumer, sc, bt).await }
    });
    let stream_stop_abort = stream_stop_handle.abort_handle();

    let reaction_action_handle = tokio::spawn({
        let sc = slack_client.clone();
        let bt = bot_token.clone();
        async move { run_reaction_action_loop(reaction_action_consumer, sc, bt).await }
    });
    let reaction_action_abort = reaction_action_handle.abort_handle();

    // stream.start handler: Core NATS request/reply — stays on raw client.
    let stream_start_handle = tokio::spawn({
        let sc = slack_client.clone();
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        async move {
            let token = SlackApiToken::new(bt.into());
            while let Some(msg) = stream_start_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("stream_start message has no reply subject, ignoring");
                        continue;
                    }
                };

                match serde_json::from_slice::<SlackStreamStartRequest>(&msg.payload) {
                    Ok(req) => {
                        let session = sc.open_session(&token);
                        let channel: SlackChannelId = req.channel.clone().into();
                        let initial_text = req.initial_text.unwrap_or_else(|| "…".to_string());

                        let mut post_req = SlackApiChatPostMessageRequest::new(
                            channel,
                            SlackMessageContent::new().with_text(initial_text),
                        );
                        if let Some(ref tts) = req.thread_ts {
                            post_req = post_req.with_thread_ts(tts.clone().into());
                        }

                        match session.chat_post_message(&post_req).await {
                            Ok(resp) => {
                                let response = SlackStreamStartResponse {
                                    channel: resp.channel.0.clone(),
                                    ts: resp.ts.0.clone(),
                                };
                                match serde_json::to_vec(&response) {
                                    Ok(bytes) => {
                                        if let Err(e) = nc.publish(reply_to, bytes.into()).await {
                                            tracing::error!(
                                                error = %e,
                                                "Failed to publish stream_start reply"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            error = %e,
                                            "Failed to serialize stream_start response"
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    "Failed to post initial stream message to Slack"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to deserialize stream_start NATS message"
                        );
                    }
                }
            }
        }
    });
    let stream_start_abort = stream_start_handle.abort_handle();

    tokio::select! {
        _ = socket_mode_listener.serve() => {
            tracing::warn!("Socket Mode listener exited");
        }
        res = outbound_handle => {
            match res {
                Ok(()) => tracing::warn!("Outbound NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Outbound NATS loop panicked"),
            }
        }
        res = stream_append_handle => {
            match res {
                Ok(()) => tracing::warn!("Stream append NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Stream append NATS loop panicked"),
            }
        }
        res = stream_stop_handle => {
            match res {
                Ok(()) => tracing::warn!("Stream stop NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Stream stop NATS loop panicked"),
            }
        }
        res = stream_start_handle => {
            match res {
                Ok(()) => tracing::warn!("Stream start NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Stream start NATS loop panicked"),
            }
        }
        res = reaction_action_handle => {
            match res {
                Ok(()) => tracing::warn!("Reaction action NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Reaction action NATS loop panicked"),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down");
        }
    }

    // Abort any still-running NATS consumer loops and give them a moment to exit.
    outbound_abort.abort();
    stream_append_abort.abort();
    stream_stop_abort.abort();
    stream_start_abort.abort();
    reaction_action_abort.abort();
    tokio::time::sleep(Duration::from_millis(200)).await;
    tracing::info!("Shutdown complete");

    Ok(())
}
