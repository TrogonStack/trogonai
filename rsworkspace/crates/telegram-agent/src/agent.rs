//! Main agent implementation

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;

use anyhow::Result;
use async_nats::Client;
use telegram_nats::{DedupStore, MessagePublisher};
use tracing::{debug, error, info, warn};

use crate::llm::ClaudeConfig;
use crate::processor::MessageProcessor;

/// Telegram agent that processes messages via a JetStream durable pull consumer.
///
/// All inbound events are consumed from the `telegram_events_{prefix}` stream
/// with at-least-once delivery: each message is ack'd after processing.
///
/// A `DedupStore` (when provided) prevents double-processing the same event in
/// the rare case where JetStream redelivers a message that was already handled
/// (e.g. crash between LLM response and ACK).  The event_id UUID from
/// `EventMetadata` is used as the stable dedup key, with a 24h TTL.
pub struct TelegramAgent {
    publisher: MessagePublisher,
    processor: MessageProcessor,
    agent_name: String,
    js: async_nats::jetstream::Context,
    prefix: String,
    dedup: Option<DedupStore>,
    /// Conversation KV — kept here so the error consumer can purge sessions
    conversation_kv: Option<async_nats::jetstream::kv::Store>,
}

impl TelegramAgent {
    /// Create a new Telegram agent
    pub fn new(
        client: Client,
        js: async_nats::jetstream::Context,
        prefix: String,
        agent_name: String,
        llm_config: Option<ClaudeConfig>,
        conversation_kv: Option<async_nats::jetstream::kv::Store>,
        dedup_kv: Option<async_nats::jetstream::kv::Store>,
    ) -> Self {
        // Use JetStream publish for outbound commands so each publish_command()
        // blocks until a PubAck is received, guaranteeing durable persistence
        // in the telegram_commands_{prefix} stream before we return to the caller.
        let publisher = MessagePublisher::with_jetstream(client, js.clone(), prefix.clone());
        let processor = MessageProcessor::new(llm_config, conversation_kv.clone());
        let dedup = dedup_kv.map(DedupStore::new);

        Self {
            publisher,
            processor,
            agent_name,
            js,
            prefix,
            dedup,
            conversation_kv,
        }
    }

    /// Run the agent: spawns the main event consumer and the error event consumer concurrently.
    pub async fn run(self) -> Result<()> {
        use std::sync::Arc;
        let agent = Arc::new(self);
        let event_task = {
            let agent = Arc::clone(&agent);
            tokio::spawn(async move { agent.run_event_consumer().await })
        };
        let error_task = {
            let agent = Arc::clone(&agent);
            tokio::spawn(async move { agent.run_error_consumer().await })
        };
        tokio::try_join!(
            async { event_task.await.map_err(|e| anyhow::anyhow!(e))? },
            async { error_task.await.map_err(|e| anyhow::anyhow!(e))? },
        )?;
        Ok(())
    }

    /// Consume inbound bot events from the JetStream stream with at-least-once delivery.
    async fn run_event_consumer(&self) -> Result<()> {
        use futures::StreamExt;
        use telegram_nats::nats::create_inbound_consumer;

        info!(
            "Agent '{}' starting event consumer (JetStream pull consumer)...",
            self.agent_name
        );

        let consumer = create_inbound_consumer(&self.js, &self.prefix, &self.agent_name).await?;
        let mut messages = consumer.messages().await?;

        info!("Agent '{}' ready, waiting for events", self.agent_name);

        while let Some(msg) = messages.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    error!("Consumer stream error: {}", e);
                    continue;
                }
            };

            let subject = msg.subject.as_str().to_string();
            let payload = msg.payload.clone();

            if let Err(e) = self.dispatch(&subject, &payload).await {
                error!("Failed to process event on '{}': {}", subject, e);
                // Still ack to avoid a stuck poison pill; the error is logged
                // and the caller can inspect dead-letter / error events separately
            }

            if let Err(e) = msg.ack().await {
                warn!("Failed to ack message on '{}': {}", subject, e);
            }
        }

        Ok(())
    }

    /// Consume `bot.error.command` events and take action by error category.
    ///
    /// - `RateLimit`    → logged; JetStream already respects the retry delay via `Nak(delay)`
    /// - `ChatMigrated` → logged with old subject + new chat_id for operator action
    /// - `BotBlocked` / `NotFound` / `PermissionDenied` → error log; session should be torn down
    /// - Other transient → warning log; JetStream will retry the original command
    pub(crate) async fn run_error_consumer(&self) -> Result<()> {
        use futures::StreamExt;
        use telegram_nats::nats::create_error_consumer;
        use telegram_types::errors::{CommandErrorEvent, ErrorCategory};

        let consumer_name = format!("{}-errors", self.agent_name);
        info!(
            "Agent '{}' starting error consumer ('{}')...",
            self.agent_name, consumer_name
        );

        let consumer = create_error_consumer(&self.js, &self.prefix, &consumer_name).await?;
        let mut messages = consumer.messages().await?;

        while let Some(msg) = messages.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    error!("Error consumer stream error: {}", e);
                    continue;
                }
            };

            match serde_json::from_slice::<CommandErrorEvent>(&msg.payload) {
                Ok(evt) => {
                    match evt.category {
                        ErrorCategory::RateLimit => {
                            warn!(
                                "Rate limit on '{}': retry after {:?}s — JetStream will redeliver after backoff",
                                evt.command_subject, evt.retry_after_secs
                            );
                        }
                        ErrorCategory::ChatMigrated => {
                            warn!(
                                "Chat migrated on '{}': new_chat_id={:?} — purging old session and letting agent rebuild",
                                evt.command_subject, evt.migrated_to_chat_id
                            );
                            // Purge the old session so the agent starts fresh with the
                            // new chat_id on the next interaction.
                            if let Some(ref session_id) = evt.session_id {
                                self.purge_session(session_id).await;
                            }
                        }
                        ErrorCategory::BotBlocked
                        | ErrorCategory::NotFound
                        | ErrorCategory::PermissionDenied => {
                            error!(
                                "Permanent failure on '{}' ({:?}): {} — purging session",
                                evt.command_subject, evt.category, evt.message
                            );
                            if let Some(ref session_id) = evt.session_id {
                                self.purge_session(session_id).await;
                            }
                        }
                        _ => {
                            warn!(
                                "Command error on '{}' ({:?}): {}",
                                evt.command_subject, evt.category, evt.message
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize CommandErrorEvent: {}", e);
                }
            }

            if let Err(e) = msg.ack().await {
                warn!("Failed to ack error event: {}", e);
            }
        }

        Ok(())
    }

    /// Delete a session's conversation history from the KV store.
    ///
    /// Called by the error consumer when a permanent Telegram error (bot blocked,
    /// chat not found, permission denied) or a chat migration is detected so the
    /// agent does not keep sending commands to a dead session.
    pub(crate) async fn purge_session(&self, session_id: &str) {
        if let Some(ref kv) = self.conversation_kv {
            let kv_key = session_id.replace(':', ".");
            match kv.delete(&kv_key).await {
                Ok(_) => info!("Purged conversation for session '{}'", session_id),
                Err(e) => warn!("Failed to purge session '{}': {}", session_id, e),
            }
        } else {
            debug!("No conversation KV — cannot purge session '{}'", session_id);
        }
    }

    /// Dispatch a message to the appropriate processor based on its subject.
    #[allow(dead_code)]
    pub(crate) async fn dispatch_for_test(&self, subject: &str, payload: &[u8]) -> Result<()> {
        self.dispatch(subject, payload).await
    }

    /// Dispatch a message to the appropriate processor based on its subject.
    ///
    /// Before processing, the `event_id` from `EventMetadata` is checked against
    /// the dedup store.  If already seen, the event is skipped silently.  On first
    /// occurrence the key is marked **before** calling the processor (optimistic
    /// locking) so that a concurrent redeliver from JetStream cannot race through.
    async fn dispatch(&self, subject: &str, payload: &[u8]) -> Result<()> {
        use telegram_nats::subjects::bot;

        let prefix = &self.prefix;
        let command_prefix = format!("telegram.{}.bot.command.", prefix);

        // ── Dedup guard ────────────────────────────────────────────────────────
        // Extract event_id from the shared EventMetadata envelope.  Any event
        // that doesn't have this field passes through (shouldn't happen in practice).
        let event_key: Option<String> = {
            #[derive(serde::Deserialize)]
            struct Envelope {
                metadata: telegram_types::events::EventMetadata,
            }
            serde_json::from_slice::<Envelope>(payload)
                .ok()
                .map(|e| format!("evt.{}", e.metadata.event_id))
        };

        if let (Some(ref dedup), Some(ref key)) = (&self.dedup, &event_key) {
            if dedup.is_seen(key).await {
                debug!("Skipping duplicate event (key={})", key);
                return Ok(());
            }
            // Mark optimistically before processing — prevents a racing redeliver
            // from triggering a second LLM call even if this one is slow.
            if let Err(e) = dedup.mark_seen(key).await {
                warn!("Failed to mark event as seen (key={}): {}", key, e);
                // Non-fatal: continue processing; at worst we get a duplicate
            }
        }
        // ── End dedup guard ────────────────────────────────────────────────────

        if subject == bot::message_text(prefix) {
            let event: telegram_types::events::MessageTextEvent = serde_json::from_slice(payload)?;
            info!(
                "Processing text message from session {}",
                event.metadata.session_id
            );
            self.processor
                .process_text_message(&event, &self.publisher)
                .await
        } else if subject == bot::message_photo(prefix) {
            let event: telegram_types::events::MessagePhotoEvent = serde_json::from_slice(payload)?;
            info!(
                "Processing photo message from session {}",
                event.metadata.session_id
            );
            self.processor
                .process_photo_message(&event, &self.publisher)
                .await
        } else if subject.starts_with(&command_prefix) {
            let event: telegram_types::events::CommandEvent = serde_json::from_slice(payload)?;
            info!(
                "Processing command /{} from session {}",
                event.command, event.metadata.session_id
            );
            self.processor
                .process_command(&event, &self.publisher)
                .await
        } else if subject == bot::callback_query(prefix) {
            let event: telegram_types::events::CallbackQueryEvent =
                serde_json::from_slice(payload)?;
            info!(
                "Processing callback query from session {}",
                event.metadata.session_id
            );
            self.processor
                .process_callback(&event, &self.publisher)
                .await
        } else if subject == bot::inline_query(prefix) {
            let event: telegram_types::events::InlineQueryEvent = serde_json::from_slice(payload)?;
            info!("Processing inline query: {}", event.query);
            self.processor
                .process_inline_query(&event, &self.publisher)
                .await
        } else {
            debug!("Unhandled subject (no processor registered): {}", subject);
            Ok(())
        }
    }
}
