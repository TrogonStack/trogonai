//! Streaming message handler with rate limiting and message tracking

use crate::outbound::{OutboundProcessor, StreamingMessage, convert_parse_mode};
use anyhow::Result;
use telegram_nats::subjects;
use telegram_types::commands::StreamMessageCommand;
use teloxide::prelude::Requester;
use teloxide::types::{ChatId, MessageId};
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Minimum time between edits to same message (Telegram rate limit)
const MIN_EDIT_INTERVAL: Duration = Duration::from_millis(1000);

/// Maximum retry attempts for failed edits
const MAX_RETRIES: u32 = 3;

impl OutboundProcessor {
    /// Handle stream message commands with rate limiting and tracking
    pub async fn handle_stream_messages(&self, prefix: String) -> Result<()> {
        let subject = subjects::agent::message_stream(&prefix);
        info!("Subscribing to streaming messages: {}", subject);

        let mut stream = self.subscriber.subscribe::<StreamMessageCommand>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    if let Err(e) = self.process_stream_message(cmd).await {
                        error!("Failed to process stream message: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize stream message command: {}", e),
            }
        }

        Ok(())
    }

    /// Process a single stream message with proper tracking
    async fn process_stream_message(&self, cmd: StreamMessageCommand) -> Result<()> {
        let chat_id = cmd.chat_id;

        // Extract session_id from message or use chat_id as fallback
        let session_id = format!("chat_{}", chat_id); // TODO: Get from metadata

        debug!(
            "Processing stream message for chat {} (final: {})",
            chat_id, cmd.is_final
        );

        // Check if we have a tracked message for this session
        let mut messages = self.streaming_messages.write().await;
        let key = (chat_id, session_id.clone());

        if let Some(tracked) = messages.get(&key) {
            // Edit existing message with rate limiting
            let message_id = tracked.message_id;
            let time_since_last_edit = tracked.last_edit.elapsed();

            // Rate limiting: wait if needed
            if time_since_last_edit < MIN_EDIT_INTERVAL {
                let wait_time = MIN_EDIT_INTERVAL - time_since_last_edit;
                debug!(
                    "Rate limiting: waiting {:?} before editing message {}",
                    wait_time, message_id
                );
                tokio::time::sleep(wait_time).await;
            }

            // Try to edit with retry logic
            if let Err(e) = self.edit_message_with_retry(
                chat_id,
                message_id,
                &cmd.text,
                cmd.parse_mode.as_ref(),
            ).await {
                error!("Failed to edit message {} after retries: {}", message_id, e);
            } else {
                // Update tracking info
                if let Some(tracked) = messages.get_mut(&key) {
                    tracked.last_edit = Instant::now();
                    tracked.edit_count += 1;
                    debug!(
                        "Updated message {} (edit #{})",
                        message_id, tracked.edit_count
                    );
                }
            }

            // Clean up if this is the final message
            if cmd.is_final {
                messages.remove(&key);
                info!("Streaming completed for chat {} (message {})", chat_id, message_id);
            }
        } else {
            // Send new message and start tracking
            match self.send_message_with_retry(
                chat_id,
                &cmd.text,
                cmd.parse_mode.as_ref(),
            ).await {
                Ok(message) => {
                    let message_id = message.id.0;
                    info!(
                        "Started streaming message {} for chat {}",
                        message_id, chat_id
                    );

                    // Track this message for future edits (unless it's already final)
                    if !cmd.is_final {
                        messages.insert(
                            key,
                            StreamingMessage {
                                message_id,
                                last_edit: Instant::now(),
                                edit_count: 0,
                            },
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to send initial streaming message: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Edit message with retry logic
    async fn edit_message_with_retry(
        &self,
        chat_id: i64,
        message_id: i32,
        text: &str,
        parse_mode: Option<&telegram_types::commands::ParseMode>,
    ) -> Result<()> {
        let mut attempts = 0;

        loop {
            attempts += 1;

            let mut req = self.bot.edit_message_text(
                ChatId(chat_id),
                MessageId(message_id),
                text,
            );

            if let Some(mode) = parse_mode {
                req.parse_mode = Some(convert_parse_mode(mode.clone()));
            }

            match req.await {
                Ok(_) => {
                    debug!("Successfully edited message {} (attempt {})", message_id, attempts);
                    return Ok(());
                }
                Err(e) => {
                    if attempts >= MAX_RETRIES {
                        return Err(anyhow::anyhow!(
                            "Failed to edit message after {} attempts: {}",
                            MAX_RETRIES,
                            e
                        ));
                    }

                    warn!(
                        "Edit failed (attempt {}): {}. Retrying...",
                        attempts, e
                    );

                    // Exponential backoff
                    let backoff = Duration::from_millis(100 * 2u64.pow(attempts - 1));
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }

    /// Send message with retry logic
    async fn send_message_with_retry(
        &self,
        chat_id: i64,
        text: &str,
        parse_mode: Option<&telegram_types::commands::ParseMode>,
    ) -> Result<teloxide::types::Message> {
        let mut attempts = 0;

        loop {
            attempts += 1;

            let mut req = self.bot.send_message(ChatId(chat_id), text);

            if let Some(mode) = parse_mode {
                req.parse_mode = Some(convert_parse_mode(mode.clone()));
            }

            match req.await {
                Ok(message) => {
                    debug!("Successfully sent message (attempt {})", attempts);
                    return Ok(message);
                }
                Err(e) => {
                    if attempts >= MAX_RETRIES {
                        return Err(anyhow::anyhow!(
                            "Failed to send message after {} attempts: {}",
                            MAX_RETRIES,
                            e
                        ));
                    }

                    warn!(
                        "Send failed (attempt {}): {}. Retrying...",
                        attempts, e
                    );

                    // Exponential backoff
                    let backoff = Duration::from_millis(100 * 2u64.pow(attempts - 1));
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}
