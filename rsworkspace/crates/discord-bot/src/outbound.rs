//! Outbound processor: NATS → Discord
//!
//! Subscribes to agent command subjects and performs the corresponding
//! Discord API calls via serenity's HTTP client.

#[path = "outbound_tests.rs"]
mod outbound_tests;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use discord_nats::{subjects, MessageSubscriber};
use discord_types::{
    AddReactionCommand, DeleteMessageCommand, EditMessageCommand, InteractionDeferCommand,
    InteractionFollowupCommand, InteractionRespondCommand, RemoveReactionCommand,
    SendMessageCommand, TypingCommand,
};
use serenity::builder::{
    CreateInteractionResponse, CreateInteractionResponseFollowup, CreateInteractionResponseMessage,
    CreateMessage, EditMessage,
};
use serenity::http::Http;
use serenity::model::id::{ChannelId, MessageId};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::outbound_streaming::StreamingMessages;

/// Processes NATS agent commands and forwards them to Discord
pub struct OutboundProcessor {
    pub(crate) http: Arc<Http>,
    pub(crate) client: async_nats::Client,
    pub(crate) prefix: String,
    pub(crate) streaming_messages: StreamingMessages,
}

impl OutboundProcessor {
    pub fn new(http: Arc<Http>, client: async_nats::Client, prefix: String) -> Self {
        Self {
            http,
            client,
            prefix,
            streaming_messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start all command handler tasks
    pub async fn run(self) -> Result<()> {
        let http = self.http;
        let client = self.client;
        let prefix = self.prefix;
        let streaming_messages = self.streaming_messages;

        info!("Starting outbound processor for prefix: {}", prefix);

        let (r1, r2, r3, r4, r5, r6, r7, r8, r9, r10) = tokio::join!(
            Self::handle_send_messages(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_messages(http.clone(), client.clone(), prefix.clone()),
            Self::handle_delete_messages(http.clone(), client.clone(), prefix.clone()),
            Self::handle_interaction_respond(http.clone(), client.clone(), prefix.clone()),
            Self::handle_interaction_defer(http.clone(), client.clone(), prefix.clone()),
            Self::handle_interaction_followup(http.clone(), client.clone(), prefix.clone()),
            Self::handle_reaction_add(http.clone(), client.clone(), prefix.clone()),
            Self::handle_reaction_remove(http.clone(), client.clone(), prefix.clone()),
            Self::handle_typing(http.clone(), client.clone(), prefix.clone()),
            crate::outbound_streaming::handle_stream_messages(
                http.clone(),
                client.clone(),
                prefix.clone(),
                streaming_messages,
            ),
        );

        // Log any errors (all handlers run indefinitely until NATS disconnects)
        for (name, result) in [
            ("send_messages", r1),
            ("edit_messages", r2),
            ("delete_messages", r3),
            ("interaction_respond", r4),
            ("interaction_defer", r5),
            ("interaction_followup", r6),
            ("reaction_add", r7),
            ("reaction_remove", r8),
            ("typing", r9),
            ("stream_messages", r10),
        ] {
            if let Err(e) = result {
                error!("Outbound handler '{}' exited with error: {}", name, e);
            }
        }

        Ok(())
    }

    async fn handle_send_messages(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_send(&prefix);
        let mut stream = subscriber.subscribe::<SendMessageCommand>(&subject).await?;

        info!("Listening for send_message commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let mut builder = CreateMessage::new().content(&cmd.content);

                    if let Some(reply_id) = cmd.reply_to_message_id {
                        builder = builder.reference_message((channel, MessageId::new(reply_id)));
                    }

                    if let Err(e) = channel.send_message(&*http, builder).await {
                        error!(
                            "Failed to send message to channel {}: {}",
                            cmd.channel_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize send_message command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_edit_messages(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_edit(&prefix);
        let mut stream = subscriber.subscribe::<EditMessageCommand>(&subject).await?;

        info!("Listening for edit_message commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_id = MessageId::new(cmd.message_id);
                    let mut builder = EditMessage::new();

                    if let Some(content) = &cmd.content {
                        builder = builder.content(content);
                    }

                    if let Err(e) = channel.edit_message(&*http, message_id, builder).await {
                        error!(
                            "Failed to edit message {} in channel {}: {}",
                            cmd.message_id, cmd.channel_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize edit_message command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_delete_messages(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_delete(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteMessageCommand>(&subject)
            .await?;

        info!("Listening for delete_message commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_id = MessageId::new(cmd.message_id);

                    if let Err(e) = channel.delete_message(&*http, message_id).await {
                        error!(
                            "Failed to delete message {} in channel {}: {}",
                            cmd.message_id, cmd.channel_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize delete_message command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_interaction_respond(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::interaction_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<InteractionRespondCommand>(&subject)
            .await?;

        info!("Listening for interaction_respond commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let mut msg = CreateInteractionResponseMessage::new();
                    if let Some(content) = &cmd.content {
                        msg = msg.content(content);
                    }
                    if cmd.ephemeral {
                        msg = msg.ephemeral(true);
                    }

                    let response = CreateInteractionResponse::Message(msg);

                    if let Err(e) = http
                        .create_interaction_response(
                            cmd.interaction_id.into(),
                            &cmd.interaction_token,
                            &response,
                            Vec::new(),
                        )
                        .await
                    {
                        error!(
                            "Failed to respond to interaction {}: {}",
                            cmd.interaction_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize interaction_respond command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_interaction_defer(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::interaction_defer(&prefix);
        let mut stream = subscriber
            .subscribe::<InteractionDeferCommand>(&subject)
            .await?;

        info!("Listening for interaction_defer commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let mut msg = CreateInteractionResponseMessage::new();
                    if cmd.ephemeral {
                        msg = msg.ephemeral(true);
                    }
                    let response = CreateInteractionResponse::Defer(msg);

                    if let Err(e) = http
                        .create_interaction_response(
                            cmd.interaction_id.into(),
                            &cmd.interaction_token,
                            &response,
                            Vec::new(),
                        )
                        .await
                    {
                        error!("Failed to defer interaction {}: {}", cmd.interaction_id, e);
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize interaction_defer command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_interaction_followup(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::interaction_followup(&prefix);
        let mut stream = subscriber
            .subscribe::<InteractionFollowupCommand>(&subject)
            .await?;

        info!("Listening for interaction_followup commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let mut builder = CreateInteractionResponseFollowup::new();
                    if let Some(content) = &cmd.content {
                        builder = builder.content(content);
                    }
                    if cmd.ephemeral {
                        builder = builder.ephemeral(true);
                    }

                    if let Err(e) = http
                        .create_followup_message(&cmd.interaction_token, &builder, Vec::new())
                        .await
                    {
                        error!(
                            "Failed to send followup for token {}: {}",
                            cmd.interaction_token, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize interaction_followup command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_reaction_add(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::reaction_add(&prefix);
        let mut stream = subscriber.subscribe::<AddReactionCommand>(&subject).await?;

        info!("Listening for reaction_add commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_id = MessageId::new(cmd.message_id);
                    let reaction = parse_reaction_type(&cmd.emoji);

                    if let Err(e) = http.create_reaction(channel, message_id, &reaction).await {
                        error!(
                            "Failed to add reaction '{}' to message {}: {}",
                            cmd.emoji, cmd.message_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize reaction_add command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_reaction_remove(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::reaction_remove(&prefix);
        let mut stream = subscriber
            .subscribe::<RemoveReactionCommand>(&subject)
            .await?;

        info!("Listening for reaction_remove commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_id = MessageId::new(cmd.message_id);
                    let reaction = parse_reaction_type(&cmd.emoji);

                    if let Err(e) = http
                        .delete_reaction_me(channel, message_id, &reaction)
                        .await
                    {
                        error!(
                            "Failed to remove reaction '{}' from message {}: {}",
                            cmd.emoji, cmd.message_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize reaction_remove command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_typing(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::channel_typing(&prefix);
        let mut stream = subscriber.subscribe::<TypingCommand>(&subject).await?;

        info!("Listening for typing commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    if let Err(e) = http.broadcast_typing(cmd.channel_id.into()).await {
                        error!(
                            "Failed to broadcast typing in channel {}: {}",
                            cmd.channel_id, e
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize typing command: {}", e);
                }
            }
        }

        Ok(())
    }
}

/// Parse an emoji string into a serenity ReactionType.
///
/// Supports:
/// - Unicode emoji: `👍`
/// - Custom emoji: `name:id` (e.g., `wave:123456789`)
fn parse_reaction_type(emoji: &str) -> serenity::model::channel::ReactionType {
    if let Some((name, id_str)) = emoji.split_once(':') {
        if let Ok(id) = id_str.parse::<u64>() {
            return serenity::model::channel::ReactionType::Custom {
                animated: false,
                id: serenity::model::id::EmojiId::new(id),
                name: Some(name.to_string()),
            };
        }
    }
    serenity::model::channel::ReactionType::Unicode(emoji.to_string())
}
