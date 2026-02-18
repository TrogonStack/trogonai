//! Main agent implementation

use anyhow::Result;
use async_nats::Client;
use discord_nats::{MessagePublisher, MessageSubscriber};
use tracing::{error, info};

use crate::llm::ClaudeConfig;
use crate::processor::{MessageProcessor, WelcomeConfig};

/// Discord agent that processes messages
pub struct DiscordAgent {
    subscriber: MessageSubscriber,
    publisher: MessagePublisher,
    processor: MessageProcessor,
    agent_name: String,
}

impl DiscordAgent {
    /// Create a new Discord agent
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: Client,
        prefix: String,
        agent_name: String,
        llm_config: Option<ClaudeConfig>,
        conversation_kv: Option<async_nats::jetstream::kv::Store>,
        system_prompt: Option<String>,
        welcome: Option<WelcomeConfig>,
        farewell: Option<WelcomeConfig>,
    ) -> Self {
        let subscriber = MessageSubscriber::new(client.clone(), prefix.clone());
        let publisher = MessagePublisher::new(client, prefix);
        let processor = MessageProcessor::new(
            llm_config,
            conversation_kv,
            system_prompt,
            welcome,
            farewell,
        );

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
            self.handle_member_events(),
            self.handle_message_lifecycle(),
            self.handle_reactions(),
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

    /// Handle guild member join and leave events
    async fn handle_member_events(&self) -> Result<()> {
        use discord_types::events::{GuildMemberAddEvent, GuildMemberRemoveEvent};

        let add_subject = discord_nats::subjects::bot::guild_member_add(self.subscriber.prefix());
        let remove_subject =
            discord_nats::subjects::bot::guild_member_remove(self.subscriber.prefix());

        info!(
            "Subscribing to member events: {}, {}",
            add_subject, remove_subject
        );

        let mut add_stream = self
            .subscriber
            .queue_subscribe::<GuildMemberAddEvent>(&add_subject, "discord-agents")
            .await?;

        let mut remove_stream = self
            .subscriber
            .queue_subscribe::<GuildMemberRemoveEvent>(&remove_subject, "discord-agents")
            .await?;

        loop {
            tokio::select! {
                Some(result) = add_stream.next() => {
                    match result {
                        Ok(event) => {
                            info!(
                                "Member joined guild {}: {}",
                                event.guild_id, event.member.user.username
                            );
                            if let Err(e) = self
                                .processor
                                .process_member_add(&event, &self.publisher)
                                .await
                            {
                                error!("Failed to process member add: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize member add event: {}", e),
                    }
                }
                Some(result) = remove_stream.next() => {
                    match result {
                        Ok(event) => {
                            info!(
                                "Member left guild {}: {}",
                                event.guild_id, event.user.username
                            );
                            if let Err(e) = self
                                .processor
                                .process_member_remove(&event, &self.publisher)
                                .await
                            {
                                error!("Failed to process member remove: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize member remove event: {}", e),
                    }
                }
                else => break,
            }
        }

        Ok(())
    }

    /// Handle message edit and delete events
    async fn handle_message_lifecycle(&self) -> Result<()> {
        use discord_types::events::{MessageDeletedEvent, MessageUpdatedEvent};

        let updated_subject =
            discord_nats::subjects::bot::message_updated(self.subscriber.prefix());
        let deleted_subject =
            discord_nats::subjects::bot::message_deleted(self.subscriber.prefix());

        info!(
            "Subscribing to message lifecycle: {}, {}",
            updated_subject, deleted_subject
        );

        let mut updated_stream = self
            .subscriber
            .queue_subscribe::<MessageUpdatedEvent>(&updated_subject, "discord-agents")
            .await?;

        let mut deleted_stream = self
            .subscriber
            .queue_subscribe::<MessageDeletedEvent>(&deleted_subject, "discord-agents")
            .await?;

        loop {
            tokio::select! {
                Some(result) = updated_stream.next() => {
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_message_updated(&event).await {
                                error!("Failed to process message update: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize message updated event: {}", e),
                    }
                }
                Some(result) = deleted_stream.next() => {
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_message_deleted(&event).await {
                                error!("Failed to process message delete: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize message deleted event: {}", e),
                    }
                }
                else => break,
            }
        }

        Ok(())
    }

    /// Handle emoji reaction add and remove events
    async fn handle_reactions(&self) -> Result<()> {
        use discord_types::events::{ReactionAddEvent, ReactionRemoveEvent};

        let add_subject = discord_nats::subjects::bot::reaction_add(self.subscriber.prefix());
        let remove_subject = discord_nats::subjects::bot::reaction_remove(self.subscriber.prefix());

        info!(
            "Subscribing to reactions: {}, {}",
            add_subject, remove_subject
        );

        let mut add_stream = self
            .subscriber
            .queue_subscribe::<ReactionAddEvent>(&add_subject, "discord-agents")
            .await?;

        let mut remove_stream = self
            .subscriber
            .queue_subscribe::<ReactionRemoveEvent>(&remove_subject, "discord-agents")
            .await?;

        loop {
            tokio::select! {
                Some(result) = add_stream.next() => {
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_reaction_add(&event).await {
                                error!("Failed to process reaction add: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize reaction add event: {}", e),
                    }
                }
                Some(result) = remove_stream.next() => {
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_reaction_remove(&event).await {
                                error!("Failed to process reaction remove: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize reaction remove event: {}", e),
                    }
                }
                else => break,
            }
        }

        Ok(())
    }
}
