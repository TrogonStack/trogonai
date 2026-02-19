//! Main agent implementation

use anyhow::Result;
use discord_nats::{MessagePublisher, MessageSubscriber, Publish, QueueSubscribeClient};
use tracing::{error, info};

use crate::health::AgentMetrics;
use crate::llm::ClaudeConfig;
use crate::processor::{MessageProcessor, WelcomeConfig};
use tokio::time::Duration;

/// Discord agent that processes messages
pub struct DiscordAgent<
    N: QueueSubscribeClient + Clone = async_nats::Client,
    P: Publish = MessagePublisher,
> {
    subscriber: MessageSubscriber<N>,
    publisher: P,
    processor: MessageProcessor,
    agent_name: String,
}

impl<N: QueueSubscribeClient + Clone, P: Publish> DiscordAgent<N, P> {
    /// Create a new Discord agent
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: N,
        prefix: String,
        publisher: P,
        agent_name: String,
        llm_config: Option<ClaudeConfig>,
        conversation_kv: Option<async_nats::jetstream::kv::Store>,
        system_prompt: Option<String>,
        welcome: Option<WelcomeConfig>,
        farewell: Option<WelcomeConfig>,
        conversation_ttl: Option<Duration>,
        metrics: Option<AgentMetrics>,
        max_history: usize,
        stream_timeout_secs: u64,
        ack_emoji: Option<String>,
    ) -> Self {
        let subscriber = MessageSubscriber::new(client, prefix);
        let processor = MessageProcessor::new(
            llm_config,
            conversation_kv,
            system_prompt,
            welcome,
            farewell,
            conversation_ttl,
            metrics,
            max_history,
            Duration::from_secs(stream_timeout_secs),
            ack_emoji,
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
            self.handle_typing(),
            self.handle_voice(),
            self.handle_guild_lifecycle(),
            self.handle_channel_lifecycle(),
            self.handle_role_lifecycle(),
            self.handle_presence(),
            self.handle_bot_ready(),
            self.handle_autocomplete(),
            self.handle_modal_submit(),
            self.handle_command_errors(),
            self.handle_guild_member_update(),
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
                result = add_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: guild_member_add");
                        break;
                    };
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
                result = remove_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: guild_member_remove");
                        break;
                    };
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
                result = updated_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: message_updated");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_message_updated(&event).await {
                                error!("Failed to process message update: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize message updated event: {}", e),
                    }
                }
                result = deleted_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: message_deleted");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_message_deleted(&event).await {
                                error!("Failed to process message delete: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize message deleted event: {}", e),
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle typing start events
    async fn handle_typing(&self) -> Result<()> {
        use discord_types::events::TypingStartEvent;

        let subject = discord_nats::subjects::bot::typing_start(self.subscriber.prefix());
        info!("Subscribing to typing events: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<TypingStartEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    if let Err(e) = self.processor.process_typing_start(&event).await {
                        error!("Failed to process typing_start: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize typing_start event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle voice state update events
    async fn handle_voice(&self) -> Result<()> {
        use discord_types::events::VoiceStateUpdateEvent;

        let subject = discord_nats::subjects::bot::voice_state_update(self.subscriber.prefix());
        info!("Subscribing to voice state events: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<VoiceStateUpdateEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    if let Err(e) = self.processor.process_voice_state_update(&event).await {
                        error!("Failed to process voice_state_update: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize voice_state_update event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle guild create, update, and delete events
    async fn handle_guild_lifecycle(&self) -> Result<()> {
        use discord_types::events::{GuildCreateEvent, GuildDeleteEvent, GuildUpdateEvent};

        let create_subject = discord_nats::subjects::bot::guild_create(self.subscriber.prefix());
        let update_subject = discord_nats::subjects::bot::guild_update(self.subscriber.prefix());
        let delete_subject = discord_nats::subjects::bot::guild_delete(self.subscriber.prefix());

        info!("Subscribing to guild lifecycle: {}, {}, {}", create_subject, update_subject, delete_subject);

        let mut create_stream = self
            .subscriber
            .queue_subscribe::<GuildCreateEvent>(&create_subject, "discord-agents")
            .await?;
        let mut update_stream = self
            .subscriber
            .queue_subscribe::<GuildUpdateEvent>(&update_subject, "discord-agents")
            .await?;
        let mut delete_stream = self
            .subscriber
            .queue_subscribe::<GuildDeleteEvent>(&delete_subject, "discord-agents")
            .await?;

        loop {
            tokio::select! {
                result = create_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: guild_create");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_guild_create(&event).await {
                                error!("Failed to process guild_create: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize guild_create event: {}", e),
                    }
                }
                result = update_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: guild_update");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_guild_update(&event).await {
                                error!("Failed to process guild_update: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize guild_update event: {}", e),
                    }
                }
                result = delete_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: guild_delete");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_guild_delete(&event).await {
                                error!("Failed to process guild_delete: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize guild_delete event: {}", e),
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle channel create, update, and delete events
    async fn handle_channel_lifecycle(&self) -> Result<()> {
        use discord_types::events::{ChannelCreateEvent, ChannelDeleteEvent, ChannelUpdateEvent};

        let create_subject = discord_nats::subjects::bot::channel_create(self.subscriber.prefix());
        let update_subject = discord_nats::subjects::bot::channel_update(self.subscriber.prefix());
        let delete_subject = discord_nats::subjects::bot::channel_delete(self.subscriber.prefix());

        info!("Subscribing to channel lifecycle: {}, {}, {}", create_subject, update_subject, delete_subject);

        let mut create_stream = self
            .subscriber
            .queue_subscribe::<ChannelCreateEvent>(&create_subject, "discord-agents")
            .await?;
        let mut update_stream = self
            .subscriber
            .queue_subscribe::<ChannelUpdateEvent>(&update_subject, "discord-agents")
            .await?;
        let mut delete_stream = self
            .subscriber
            .queue_subscribe::<ChannelDeleteEvent>(&delete_subject, "discord-agents")
            .await?;

        loop {
            tokio::select! {
                result = create_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: channel_create");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_channel_create(&event).await {
                                error!("Failed to process channel_create: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize channel_create event: {}", e),
                    }
                }
                result = update_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: channel_update");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_channel_update(&event).await {
                                error!("Failed to process channel_update: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize channel_update event: {}", e),
                    }
                }
                result = delete_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: channel_delete");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_channel_delete(&event).await {
                                error!("Failed to process channel_delete: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize channel_delete event: {}", e),
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle role create, update, and delete events
    async fn handle_role_lifecycle(&self) -> Result<()> {
        use discord_types::events::{RoleCreateEvent, RoleDeleteEvent, RoleUpdateEvent};

        let create_subject = discord_nats::subjects::bot::role_create(self.subscriber.prefix());
        let update_subject = discord_nats::subjects::bot::role_update(self.subscriber.prefix());
        let delete_subject = discord_nats::subjects::bot::role_delete(self.subscriber.prefix());

        info!("Subscribing to role lifecycle: {}, {}, {}", create_subject, update_subject, delete_subject);

        let mut create_stream = self
            .subscriber
            .queue_subscribe::<RoleCreateEvent>(&create_subject, "discord-agents")
            .await?;
        let mut update_stream = self
            .subscriber
            .queue_subscribe::<RoleUpdateEvent>(&update_subject, "discord-agents")
            .await?;
        let mut delete_stream = self
            .subscriber
            .queue_subscribe::<RoleDeleteEvent>(&delete_subject, "discord-agents")
            .await?;

        loop {
            tokio::select! {
                result = create_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: role_create");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_role_create(&event).await {
                                error!("Failed to process role_create: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize role_create event: {}", e),
                    }
                }
                result = update_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: role_update");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_role_update(&event).await {
                                error!("Failed to process role_update: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize role_update event: {}", e),
                    }
                }
                result = delete_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: role_delete");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self.processor.process_role_delete(&event).await {
                                error!("Failed to process role_delete: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize role_delete event: {}", e),
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle presence update events
    async fn handle_presence(&self) -> Result<()> {
        use discord_types::events::PresenceUpdateEvent;

        let subject = discord_nats::subjects::bot::presence_update(self.subscriber.prefix());
        info!("Subscribing to presence events: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<PresenceUpdateEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    if let Err(e) = self.processor.process_presence_update(&event).await {
                        error!("Failed to process presence_update: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize presence_update event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle bot ready events
    async fn handle_bot_ready(&self) -> Result<()> {
        use discord_types::events::BotReadyEvent;

        let subject = discord_nats::subjects::bot::bot_ready(self.subscriber.prefix());
        info!("Subscribing to bot_ready: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<BotReadyEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    if let Err(e) = self.processor.process_bot_ready(&event).await {
                        error!("Failed to process bot_ready: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize bot_ready event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle autocomplete interaction events
    async fn handle_autocomplete(&self) -> Result<()> {
        use discord_types::events::AutocompleteEvent;

        let subject = discord_nats::subjects::bot::interaction_autocomplete(self.subscriber.prefix());
        info!("Subscribing to autocomplete: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<AutocompleteEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    if let Err(e) = self
                        .processor
                        .process_autocomplete(&event, &self.publisher)
                        .await
                    {
                        error!("Failed to process autocomplete: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize autocomplete event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle modal submit events
    async fn handle_modal_submit(&self) -> Result<()> {
        use discord_types::events::ModalSubmitEvent;

        let subject = discord_nats::subjects::bot::interaction_modal(self.subscriber.prefix());
        info!("Subscribing to modal_submit: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<ModalSubmitEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    if let Err(e) = self
                        .processor
                        .process_modal_submit(&event, &self.publisher)
                        .await
                    {
                        error!("Failed to process modal_submit: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize modal_submit event: {}", e),
            }
        }

        Ok(())
    }

    /// Handle command error events published by the bot
    async fn handle_command_errors(&self) -> Result<()> {
        let subject = discord_nats::subjects::bot::command_error(self.subscriber.prefix());
        info!("Subscribing to command errors: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<serde_json::Value>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(payload) => {
                    self.processor.process_command_error(payload).await;
                }
                Err(e) => {
                    tracing::warn!("bad command_error payload: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Handle guild member update events
    async fn handle_guild_member_update(&self) -> Result<()> {
        use discord_types::events::GuildMemberUpdateEvent;

        let subject = discord_nats::subjects::bot::guild_member_update(self.subscriber.prefix());
        info!("Subscribing to guild member update: {}", subject);

        let mut stream = self
            .subscriber
            .queue_subscribe::<GuildMemberUpdateEvent>(&subject, "discord-agents")
            .await?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    self.processor.process_guild_member_update(&event).await;
                }
                Err(e) => error!("Failed to deserialize guild_member_update event: {}", e),
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
                result = add_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: reaction_add");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self
                                .processor
                                .process_reaction_add(&event, &self.publisher)
                                .await
                            {
                                error!("Failed to process reaction add: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize reaction add event: {}", e),
                    }
                }
                result = remove_stream.next() => {
                    let Some(result) = result else {
                        tracing::warn!("subscription closed: reaction_remove");
                        break;
                    };
                    match result {
                        Ok(event) => {
                            if let Err(e) = self
                                .processor
                                .process_reaction_remove(&event, &self.publisher)
                                .await
                            {
                                error!("Failed to process reaction remove: {}", e);
                            }
                        }
                        Err(e) => error!("Failed to deserialize reaction remove event: {}", e),
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discord_nats::{MockNatsClient, MockPublisher};

    fn make_agent() -> DiscordAgent<MockNatsClient, MockPublisher> {
        let client = MockNatsClient::new();
        let publisher = MockPublisher::new("test");
        DiscordAgent::new(
            client, "test".to_string(), publisher, "test-agent".to_string(),
            None, None, None, None, None, None, None, 20, 120, None,
        )
    }

    fn make_message_event(
        session_id: &str,
        content: &str,
        channel_id: u64,
        message_id: u64,
    ) -> discord_types::events::MessageCreatedEvent {
        use discord_types::events::{EventMetadata, MessageCreatedEvent};
        use discord_types::types::{DiscordMessage, DiscordUser};
        MessageCreatedEvent {
            metadata: EventMetadata::new(session_id, 1),
            message: DiscordMessage {
                id: message_id,
                channel_id,
                guild_id: None,
                author: DiscordUser { id: 42, username: "tester".to_string(), global_name: None, bot: false },
                content: content.to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                edited_timestamp: None,
                attachments: vec![],
                embeds: vec![],
                referenced_message_id: None,
                referenced_message_content: None,
            },
            pluralkit_member_id: None,
            pluralkit_member_name: None,
        }
    }

    /// Verify the agent forwards prefix and name correctly.
    #[test]
    fn test_agent_constructs_with_mock() {
        let agent = make_agent();
        assert_eq!(agent.agent_name, "test-agent");
        assert_eq!(agent.subscriber.prefix(), "test");
    }

    /// run() must propagate the subscribe error immediately when the NATS client is a mock.
    #[tokio::test]
    async fn test_agent_run_returns_err_when_subscribe_fails() {
        let agent = make_agent();
        let result = agent.run().await;
        assert!(result.is_err(), "run() must return Err when subscribe fails");
        let msg = result.unwrap_err().to_string();
        assert!(!msg.is_empty(), "error must have a message");
    }

    /// Without ack_emoji: process_message publishes exactly typing + send_message (2 commands)
    /// and the send_message carries the original content and channel_id.
    #[tokio::test]
    async fn test_agent_echo_mode_publishes_typing_and_send() {
        let agent = make_agent();
        let pub_ref = MockPublisher::new("test");
        let event = make_message_event("sess-1", "hello world", 100, 50);

        agent.processor.process_message(&event, &pub_ref).await.unwrap();

        let msgs = pub_ref.published_messages();
        assert_eq!(msgs.len(), 2, "echo mode must publish typing + send_message");
        let cmd: discord_types::SendMessageCommand =
            serde_json::from_value(msgs[1].1.clone()).unwrap();
        assert_eq!(cmd.channel_id, 100, "send must target the right channel");
        assert!(cmd.content.contains("hello world"), "send must echo the content");
        assert_eq!(cmd.reply_to_message_id, Some(50), "send must reply to the original message");
    }

    /// With ack_emoji: process_message publishes ack_reaction first, then typing, then send_message.
    /// The ack reaction must carry the configured emoji and target the original message.
    #[tokio::test]
    async fn test_agent_with_ack_emoji_publishes_ack_reaction_first() {
        let client = MockNatsClient::new();
        let publisher = MockPublisher::new("test");
        let agent = DiscordAgent::new(
            client, "test".to_string(), publisher, "ack-agent".to_string(),
            None, None, None, None, None, None, None, 20, 120, Some("⏳".to_string()),
        );
        let pub_ref = MockPublisher::new("test");
        let event = make_message_event("sess-2", "help me", 200, 99);

        agent.processor.process_message(&event, &pub_ref).await.unwrap();

        let msgs = pub_ref.published_messages();
        assert!(msgs.len() >= 3, "with ack_emoji: must publish ack_reaction + typing + send_message");
        let reaction: discord_types::AddReactionCommand =
            serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(reaction.emoji, "⏳", "ack reaction must use the configured emoji");
        assert_eq!(reaction.message_id, 99, "ack reaction must target the original message");
        assert_eq!(reaction.channel_id, 200, "ack reaction must be in the right channel");
    }

    /// Two agents with different prefixes must not share publisher state.
    #[tokio::test]
    async fn test_agent_publishers_are_independent() {
        let pub_a = MockPublisher::new("pfx-a");
        let pub_b = MockPublisher::new("pfx-b");
        let agent_a = DiscordAgent::new(
            MockNatsClient::new(), "pfx-a".to_string(), pub_a.clone(), "agent-a".to_string(),
            None, None, None, None, None, None, None, 20, 120, None,
        );
        let agent_b = DiscordAgent::new(
            MockNatsClient::new(), "pfx-b".to_string(), pub_b.clone(), "agent-b".to_string(),
            None, None, None, None, None, None, None, 20, 120, None,
        );

        let event = make_message_event("sess-a", "ping", 100, 1);
        agent_a.processor.process_message(&event, &pub_a).await.unwrap();

        assert!(!pub_a.is_empty(), "agent_a's publisher must have messages");
        assert!(pub_b.is_empty(), "agent_b's publisher must be unaffected");
        assert_ne!(agent_a.subscriber.prefix(), agent_b.subscriber.prefix());
    }
}
