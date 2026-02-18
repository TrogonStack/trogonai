//! Streaming message handler with rate limiting and session tracking.
//!
//! Subscribes to `discord.{prefix}.agent.message.stream` and progressively
//! edits a Discord message as LLM chunks arrive.
//!
//! Flow:
//! 1. First `StreamMessageCommand` for a `session_id` → send a new message,
//!    track `session_id → (channel_id, message_id)`.
//! 2. Subsequent commands → edit the tracked message (rate-limited).
//! 3. `is_final = true` → final edit + remove from tracking state.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use discord_nats::subjects;
use discord_types::StreamMessageCommand;
use serenity::builder::{CreateMessage, EditMessage};
use serenity::http::Http;
use serenity::model::id::{ChannelId, MessageId};
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Minimum time between edits to the same message.
/// Discord allows ~5 edits/s per channel; 400 ms gives comfortable headroom.
const MIN_EDIT_INTERVAL: Duration = Duration::from_millis(400);

/// Maximum retry attempts for a failed API call.
const MAX_RETRIES: u32 = 3;

/// Tracking state for an in-progress streamed message.
pub(crate) struct StreamingState {
    pub channel_id: u64,
    pub message_id: u64,
    pub last_edit: Instant,
    pub edit_count: u64,
}

/// Shared map: `session_id → StreamingState`
pub(crate) type StreamingMessages = Arc<RwLock<HashMap<String, StreamingState>>>;

/// Subscribe to stream commands and apply them to Discord messages.
pub(crate) async fn handle_stream_messages(
    http: Arc<Http>,
    client: async_nats::Client,
    prefix: String,
    streaming_messages: StreamingMessages,
) -> Result<()> {
    use discord_nats::MessageSubscriber;

    let subscriber = MessageSubscriber::new(client, &prefix);
    let subject = subjects::agent::message_stream(&prefix);
    let mut stream = subscriber
        .subscribe::<StreamMessageCommand>(&subject)
        .await?;

    info!("Listening for stream_message commands on {}", subject);

    while let Some(result) = stream.next().await {
        match result {
            Ok(cmd) => {
                if let Err(e) = process_stream_message(&http, &streaming_messages, cmd).await {
                    error!("Failed to process stream_message: {}", e);
                }
            }
            Err(e) => warn!("Failed to deserialize stream_message command: {}", e),
        }
    }

    Ok(())
}

async fn process_stream_message(
    http: &Http,
    streaming_messages: &StreamingMessages,
    cmd: StreamMessageCommand,
) -> Result<()> {
    let session_id = &cmd.session_id;

    // -- look up existing tracked message -----------------------------------
    let existing = {
        let map = streaming_messages.read().await;
        map.get(session_id)
            .map(|s| (s.channel_id, s.message_id, s.last_edit, s.edit_count))
    };

    if let Some((channel_id, message_id, last_edit, edit_count)) = existing {
        // Rate limit: wait if the last edit was too recent
        let elapsed = last_edit.elapsed();
        if elapsed < MIN_EDIT_INTERVAL {
            tokio::time::sleep(MIN_EDIT_INTERVAL - elapsed).await;
        }

        // Edit the existing message
        if let Err(e) = edit_message_with_retry(http, channel_id, message_id, &cmd.content).await {
            error!("Failed to edit message {} after retries: {}", message_id, e);
        } else {
            let new_edit_count = edit_count + 1;
            debug!(
                "Edited streaming message {} (edit #{})",
                message_id, new_edit_count
            );

            if cmd.is_final {
                streaming_messages.write().await.remove(session_id);
                info!(
                    "Streaming complete for session {} (message {})",
                    session_id, message_id
                );
            } else if let Some(state) = streaming_messages.write().await.get_mut(session_id) {
                state.last_edit = Instant::now();
                state.edit_count = new_edit_count;
            }
        }
    } else {
        // No tracked message yet → send the first one
        match send_initial_message(http, &cmd).await {
            Ok(new_message_id) => {
                info!(
                    "Started streaming message {} for session {}",
                    new_message_id, session_id
                );

                if !cmd.is_final {
                    streaming_messages.write().await.insert(
                        session_id.clone(),
                        StreamingState {
                            channel_id: cmd.channel_id,
                            message_id: new_message_id,
                            last_edit: Instant::now(),
                            edit_count: 0,
                        },
                    );
                }
            }
            Err(e) => error!("Failed to send initial streaming message: {}", e),
        }
    }

    Ok(())
}

/// Send the initial message (with optional reply) and return the new message ID.
async fn send_initial_message(http: &Http, cmd: &StreamMessageCommand) -> Result<u64> {
    let channel = ChannelId::new(cmd.channel_id);
    let mut builder = CreateMessage::new().content(&cmd.content);

    if let Some(reply_id) = cmd.reply_to_message_id {
        builder = builder.reference_message((channel, MessageId::new(reply_id)));
    }

    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match channel.send_message(http, builder.clone()).await {
            Ok(msg) => return Ok(msg.id.get()),
            Err(e) if attempts < MAX_RETRIES => {
                warn!("Send failed (attempt {}): {}. Retrying...", attempts, e);
                tokio::time::sleep(Duration::from_millis(100 * 2u64.pow(attempts - 1))).await;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to send message after {} attempts: {}",
                    MAX_RETRIES,
                    e
                ))
            }
        }
    }
}

/// Edit a message with exponential-backoff retry.
async fn edit_message_with_retry(
    http: &Http,
    channel_id: u64,
    message_id: u64,
    content: &str,
) -> Result<()> {
    let channel = ChannelId::new(channel_id);
    let msg_id = MessageId::new(message_id);
    let mut attempts = 0u32;

    loop {
        attempts += 1;
        let builder = EditMessage::new().content(content);
        match channel.edit_message(http, msg_id, builder).await {
            Ok(_) => {
                debug!("Edited message {} (attempt {})", message_id, attempts);
                return Ok(());
            }
            Err(e) if attempts < MAX_RETRIES => {
                warn!("Edit failed (attempt {}): {}. Retrying...", attempts, e);
                tokio::time::sleep(Duration::from_millis(100 * 2u64.pow(attempts - 1))).await;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to edit message {} after {} attempts: {}",
                    message_id,
                    MAX_RETRIES,
                    e
                ))
            }
        }
    }
}
