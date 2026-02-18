//! Main agent implementation

use anyhow::Result;
use async_nats::Client;
use discord_nats::{MessagePublisher, MessageSubscriber};
use tracing::{error, info};

use crate::llm::ClaudeConfig;
use crate::processor::MessageProcessor;

/// Discord agent that processes messages
pub struct DiscordAgent {
    subscriber: MessageSubscriber,
    publisher: MessagePublisher,
    processor: MessageProcessor,
    agent_name: String,
}

impl DiscordAgent {
    /// Create a new Discord agent
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

        tokio::try_join!(
            self.handle_messages(),
            self.handle_slash_commands(),
            self.handle_component_interactions(),
        )?;

        Ok(())
    }

    /// Handle regular channel messages
    async fn handle_messages(&self) -> Result<()> {
        use discord_types::events::MessageCreatedEvent;

        let subject = discord_nats::subjects::bot::message_created(self.subscriber.prefix());
        info!("Subscribing to messages: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<MessageCreatedEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received message from session {}: {}",
                        event.metadata.session_id, event.message.content
                    );
                    if let Err(e) = self
                        .processor
                        .process_message(&event, &self.publisher)
                        .await
                    {
                        error!("Failed to process message: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize message event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle slash command interactions
    async fn handle_slash_commands(&self) -> Result<()> {
        use discord_types::events::SlashCommandEvent;

        let subject = discord_nats::subjects::bot::interaction_command(self.subscriber.prefix());
        info!("Subscribing to slash commands: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<SlashCommandEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received slash command /{} from session {}",
                        event.command_name, event.metadata.session_id
                    );
                    if let Err(e) = self
                        .processor
                        .process_slash_command(&event, &self.publisher)
                        .await
                    {
                        error!("Failed to process slash command: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize slash command event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle button / select menu interactions
    async fn handle_component_interactions(&self) -> Result<()> {
        use discord_types::events::ComponentInteractionEvent;

        let subject = discord_nats::subjects::bot::interaction_component(self.subscriber.prefix());
        info!("Subscribing to component interactions: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<ComponentInteractionEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    info!(
                        "Received component interaction '{}' from session {}",
                        event.custom_id, event.metadata.session_id
                    );
                    if let Err(e) = self
                        .processor
                        .process_component_interaction(&event, &self.publisher)
                        .await
                    {
                        error!("Failed to process component interaction: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize component interaction event: {}", e),
            }
        }

        Ok(())
    }
}
