//! Main agent implementation

use anyhow::Result;
use async_nats::Client;
use telegram_nats::MessagePublisher;
use tracing::{debug, error, info, warn};

use crate::llm::ClaudeConfig;
use crate::processor::MessageProcessor;

/// Telegram agent that processes messages via a JetStream durable pull consumer.
///
/// All inbound events are consumed from the `telegram_events_{prefix}` stream
/// with at-least-once delivery: each message is ack'd after processing.
pub struct TelegramAgent {
    publisher: MessagePublisher,
    processor: MessageProcessor,
    agent_name: String,
    js: async_nats::jetstream::Context,
    prefix: String,
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
    ) -> Self {
        let publisher = MessagePublisher::new(client, prefix.clone());
        let processor = MessageProcessor::new(llm_config, conversation_kv);

        Self {
            publisher,
            processor,
            agent_name,
            js,
            prefix,
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

        let consumer =
            create_inbound_consumer(&self.js, &self.prefix, &self.agent_name).await?;
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
    async fn run_error_consumer(&self) -> Result<()> {
        use futures::StreamExt;
        use telegram_nats::nats::create_inbound_consumer;
        use telegram_types::errors::{CommandErrorEvent, ErrorCategory};

        let consumer_name = format!("{}-errors", self.agent_name);
        info!(
            "Agent '{}' starting error consumer ('{}')...",
            self.agent_name, consumer_name
        );

        let consumer =
            create_inbound_consumer(&self.js, &self.prefix, &consumer_name).await?;
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
                Ok(evt) => match evt.category {
                    ErrorCategory::RateLimit => {
                        warn!(
                            "Rate limit on '{}': retry after {:?}s — JetStream will redeliver after backoff",
                            evt.command_subject, evt.retry_after_secs
                        );
                    }
                    ErrorCategory::ChatMigrated => {
                        warn!(
                            "Chat migrated on '{}': new_chat_id={:?} — update routing",
                            evt.command_subject, evt.migrated_to_chat_id
                        );
                    }
                    ErrorCategory::BotBlocked
                    | ErrorCategory::NotFound
                    | ErrorCategory::PermissionDenied => {
                        error!(
                            "Permanent failure on '{}' ({:?}): {}",
                            evt.command_subject, evt.category, evt.message
                        );
                    }
                    _ => {
                        warn!(
                            "Command error on '{}' ({:?}): {}",
                            evt.command_subject, evt.category, evt.message
                        );
                    }
                },
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

    /// Dispatch a message to the appropriate processor based on its subject
    async fn dispatch(&self, subject: &str, payload: &[u8]) -> Result<()> {
        use telegram_nats::subjects::bot;

        let prefix = &self.prefix;
        let command_prefix = format!("telegram.{}.bot.command.", prefix);

        if subject == bot::message_text(prefix) {
            let event: telegram_types::events::MessageTextEvent =
                serde_json::from_slice(payload)?;
            info!(
                "Processing text message from session {}",
                event.metadata.session_id
            );
            self.processor
                .process_text_message(&event, &self.publisher)
                .await
        } else if subject == bot::message_photo(prefix) {
            let event: telegram_types::events::MessagePhotoEvent =
                serde_json::from_slice(payload)?;
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
            let event: telegram_types::events::InlineQueryEvent =
                serde_json::from_slice(payload)?;
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
