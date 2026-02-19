mod config;
mod handler;
mod health;
mod llm;
mod memory;

use async_nats::jetstream;
use config::SlackAgentConfig;
use futures::StreamExt;
use handler::{
    AgentContext, handle_block_action, handle_inbound, handle_message_changed,
    handle_message_deleted, handle_pin, handle_reaction, handle_slash_command,
    handle_thread_broadcast,
};
use health::start_health_server;
use llm::ClaudeClient;
use memory::ConversationMemory;
use slack_nats::setup::ensure_slack_stream;
use slack_nats::subscriber::{
    create_block_action_consumer, create_inbound_consumer, create_message_changed_consumer,
    create_message_deleted_consumer, create_pin_consumer, create_reaction_consumer,
    create_slash_command_consumer, create_thread_broadcast_consumer,
};
use slack_types::events::{
    SlackBlockActionEvent, SlackInboundMessage, SlackMessageChangedEvent, SlackMessageDeletedEvent,
    SlackPinEvent, SlackReactionEvent, SlackSlashCommandEvent, SlackThreadBroadcastEvent,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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

    let config = SlackAgentConfig::from_env(&SystemEnv);

    tracing::info!(port = config.health_port, "Starting health check server...");
    tokio::spawn(start_health_server(config.health_port));

    tracing::info!("Connecting to NATS...");
    let nats_client = connect(&config.nats).await.map_err(|e| format!("{:?}", e))?;
    // Keep raw client for Core NATS request/reply (stream.start).
    let nats_raw = nats_client.clone();
    let js = Arc::new(jetstream::new(nats_client));

    tracing::info!("Setting up JetStream stream and KV bucket...");
    ensure_slack_stream(&js).await?;
    let memory = ConversationMemory::new(&js, config.claude_max_history).await?;

    tracing::info!("Creating JetStream consumers...");
    let inbound_consumer = create_inbound_consumer(&js).await?;
    let reaction_consumer = create_reaction_consumer(&js).await?;
    let changed_consumer = create_message_changed_consumer(&js).await?;
    let deleted_consumer = create_message_deleted_consumer(&js).await?;
    let slash_consumer = create_slash_command_consumer(&js).await?;
    let pin_consumer = create_pin_consumer(&js).await?;
    let block_action_consumer = create_block_action_consumer(&js).await?;
    let thread_broadcast_consumer = create_thread_broadcast_consumer(&js).await?;

    let mut inbound_msgs = inbound_consumer.messages().await?;
    let mut reaction_msgs = reaction_consumer.messages().await?;
    let mut changed_msgs = changed_consumer.messages().await?;
    let mut deleted_msgs = deleted_consumer.messages().await?;
    let mut slash_msgs = slash_consumer.messages().await?;
    let mut pin_msgs = pin_consumer.messages().await?;
    let mut block_action_msgs = block_action_consumer.messages().await?;
    let mut thread_broadcast_msgs = thread_broadcast_consumer.messages().await?;

    // Build Claude client (optional — only if API key is configured).
    let claude = config.anthropic_api_key.as_ref().map(|key| {
        tracing::info!(model = %config.claude_model, "Claude client configured");
        ClaudeClient::new(
            key.clone(),
            config.claude_model.clone(),
            config.claude_max_tokens,
            config.claude_system_prompt.clone(),
        )
    });

    if claude.is_none() {
        tracing::warn!("ANTHROPIC_API_KEY not set — agent will echo messages instead of calling Claude");
    }

    let ctx = Arc::new(AgentContext {
        js: js.clone(),
        nats: nats_raw,
        claude,
        memory,
        config: config.clone(),
        session_locks: Mutex::new(HashMap::new()),
    });

    tracing::info!("Slack agent running. Press Ctrl+C to stop.");

    loop {
        tokio::select! {
            Some(result) = inbound_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackInboundMessage>(&msg.payload) {
                            Ok(ev) => {
                                let ctx = Arc::clone(&ctx);
                                tokio::spawn(async move { handle_inbound(ev, ctx).await });
                            }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackInboundMessage"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on inbound consumer"),
                }
            }
            Some(result) = reaction_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackReactionEvent>(&msg.payload) {
                            Ok(ev) => { handle_reaction(ev).await; }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackReactionEvent"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on reaction consumer"),
                }
            }
            Some(result) = changed_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackMessageChangedEvent>(&msg.payload) {
                            Ok(ev) => { handle_message_changed(ev).await; }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackMessageChangedEvent"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on message_changed consumer"),
                }
            }
            Some(result) = deleted_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackMessageDeletedEvent>(&msg.payload) {
                            Ok(ev) => { handle_message_deleted(ev).await; }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackMessageDeletedEvent"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on message_deleted consumer"),
                }
            }
            Some(result) = slash_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackSlashCommandEvent>(&msg.payload) {
                            Ok(ev) => {
                                let ctx = Arc::clone(&ctx);
                                tokio::spawn(async move { handle_slash_command(ev, ctx).await });
                            }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackSlashCommandEvent"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on slash_command consumer"),
                }
            }
            Some(result) = pin_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackPinEvent>(&msg.payload) {
                            Ok(ev) => { handle_pin(ev).await; }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackPinEvent"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on pin consumer"),
                }
            }
            Some(result) = block_action_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackBlockActionEvent>(&msg.payload) {
                            Ok(ev) => { handle_block_action(ev).await; }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackBlockActionEvent"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on block_action consumer"),
                }
            }
            Some(result) = thread_broadcast_msgs.next() => {
                match result {
                    Ok(msg) => {
                        match serde_json::from_slice::<SlackThreadBroadcastEvent>(&msg.payload) {
                            Ok(ev) => { handle_thread_broadcast(ev).await; }
                            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackThreadBroadcastEvent"),
                        }
                        let _ = msg.ack().await;
                    }
                    Err(e) => tracing::error!(error = %e, "JetStream error on thread_broadcast consumer"),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received Ctrl+C, shutting down");
                break;
            }
        }
    }

    Ok(())
}
