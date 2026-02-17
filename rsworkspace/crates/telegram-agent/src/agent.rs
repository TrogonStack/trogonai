//! Main agent implementation

use async_nats::Client;
use telegram_nats::{MessageSubscriber, MessagePublisher};
use tracing::{info, error};
use anyhow::Result;

use crate::llm::ClaudeConfig;
use crate::processor::MessageProcessor;

/// Telegram agent that processes messages
pub struct TelegramAgent {
    subscriber: MessageSubscriber,
    publisher: MessagePublisher,
    processor: MessageProcessor,
    agent_name: String,
}

impl TelegramAgent {
    /// Create a new Telegram agent
    pub fn new(
        client: Client,
        prefix: String,
        agent_name: String,
        llm_config: Option<ClaudeConfig>,
        conversation_kv: Option<async_nats::jetstream::kv::Store>,
    ) -> Self {
        let subscriber = MessageSubscriber::new(client.clone(), prefix.clone());
        let publisher = MessagePublisher::new(client, prefix);
        let processor = MessageProcessor::new(llm_config, conversation_kv);

        Self {
            subscriber,
            publisher,
            processor,
            agent_name,
        }
    }

    /// Run the agent
    pub async fn run(self) -> Result<()> {
        info!("Agent '{}' starting...", self.agent_name);

        // Spawn handlers for different message types
        let text_handler = self.handle_text_messages();
        let photo_handler = self.handle_photo_messages();
        let command_handler = self.handle_commands();
        let callback_handler = self.handle_callbacks();
        let inline_handler = self.handle_inline_queries();

        // Run all handlers concurrently
        tokio::try_join!(
            text_handler,
            photo_handler,
            command_handler,
            callback_handler,
            inline_handler
        )?;

        Ok(())
    }

    /// Handle text messages
    async fn handle_text_messages(&self) -> Result<()> {
        use telegram_types::events::MessageTextEvent;

        let subject = telegram_nats::subjects::bot::message_text(self.subscriber.prefix());
        info!("Subscribing to text messages: {}", subject);

        let mut stream = self.subscriber.subscribe::<MessageTextEvent>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received text message from session {}: {}",
                        event.metadata.session_id,
                        event.text
                    );

                    // Process the message
                    if let Err(e) = self.processor.process_text_message(
                        &event,
                        &self.publisher
                    ).await {
                        error!("Failed to process text message: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize text message event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle photo messages
    async fn handle_photo_messages(&self) -> Result<()> {
        use telegram_types::events::MessagePhotoEvent;

        let subject = telegram_nats::subjects::bot::message_photo(self.subscriber.prefix());
        info!("Subscribing to photo messages: {}", subject);

        let mut stream = self.subscriber.subscribe::<MessagePhotoEvent>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received photo message from session {}",
                        event.metadata.session_id
                    );

                    // Process the photo message
                    if let Err(e) = self.processor.process_photo_message(
                        &event,
                        &self.publisher
                    ).await {
                        error!("Failed to process photo message: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize photo message event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle commands
    async fn handle_commands(&self) -> Result<()> {
        use telegram_types::events::CommandEvent;

        // Subscribe to all commands using wildcard
        let subject = telegram_nats::subjects::bot::all_commands(self.subscriber.prefix());
        info!("Subscribing to commands: {}", subject);

        let mut stream = self.subscriber.subscribe::<CommandEvent>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received command /{} from session {}",
                        event.command,
                        event.metadata.session_id
                    );

                    // Process the command
                    if let Err(e) = self.processor.process_command(
                        &event,
                        &self.publisher
                    ).await {
                        error!("Failed to process command: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize command event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle callback queries
    async fn handle_callbacks(&self) -> Result<()> {
        use telegram_types::events::CallbackQueryEvent;

        let subject = telegram_nats::subjects::bot::callback_query(self.subscriber.prefix());
        info!("Subscribing to callback queries: {}", subject);

        let mut stream = self.subscriber.subscribe::<CallbackQueryEvent>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received callback query from session {}: {}",
                        event.metadata.session_id,
                        event.data
                    );

                    // Process the callback
                    if let Err(e) = self.processor.process_callback(
                        &event,
                        &self.publisher
                    ).await {
                        error!("Failed to process callback: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize callback query event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle inline queries
    async fn handle_inline_queries(&self) -> Result<()> {
        use telegram_types::events::InlineQueryEvent;

        let subject = telegram_nats::subjects::bot::inline_query(self.subscriber.prefix());
        info!("Subscribing to inline queries: {}", subject);

        let mut stream = self.subscriber.subscribe::<InlineQueryEvent>(&subject).await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received inline query from user {}: {}",
                        event.from.id,
                        event.query
                    );

                    // Process the inline query
                    if let Err(e) = self.processor.process_inline_query(
                        &event,
                        &self.publisher
                    ).await {
                        error!("Failed to process inline query: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize inline query event: {}", e),
            }
        }

        Ok(())
    }
}
