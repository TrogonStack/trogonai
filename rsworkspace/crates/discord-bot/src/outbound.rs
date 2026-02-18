//! Outbound processor: NATS → Discord
//!
//! Subscribes to agent command subjects and performs the corresponding
//! Discord API calls via serenity's HTTP client.

#[path = "outbound_tests.rs"]
mod outbound_tests;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use discord_nats::{subjects, MessagePublisher, MessageSubscriber};
use discord_types::{
    AddReactionCommand, ArchiveThreadCommand, AssignRoleCommand, AutocompleteRespondCommand,
    BanUserCommand, BulkDeleteMessagesCommand, CreateChannelCommand, CreateRoleCommand,
    CreateThreadCommand, DeleteChannelCommand, DeleteMessageCommand, DeleteRoleCommand,
    EditChannelCommand, EditMessageCommand, InteractionDeferCommand, InteractionFollowupCommand,
    InteractionRespondCommand, KickUserCommand, ModalRespondCommand, PinMessageCommand,
    RemoveReactionCommand, RemoveRoleCommand, SendMessageCommand, TimeoutUserCommand,
    TypingCommand, UnpinMessageCommand,
};
use serenity::builder::{
    CreateAutocompleteResponse, CreateChannel, CreateInteractionResponse,
    CreateInteractionResponseFollowup, CreateInteractionResponseMessage, CreateMessage,
    CreateThread, EditChannel, EditMember, EditMessage, EditRole, EditThread,
};
use serenity::http::Http;
use serenity::model::id::{ChannelId, GuildId, MessageId, RoleId, UserId};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::errors::{classify, log_error, log_outcome, ErrorOutcome};

use crate::outbound_streaming::{StreamingMessages, StreamingState};

/// Discord's maximum message content length in characters.
const MAX_DISCORD_LEN: usize = 2000;

/// Maximum retry attempts for rate-limited Discord API calls.
const MAX_RETRIES: u32 = 3;

/// Truncate a string to Discord's 2000-character limit.
fn truncate(s: &str) -> &str {
    if s.len() <= MAX_DISCORD_LEN {
        s
    } else {
        let mut end = MAX_DISCORD_LEN - 1;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

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
        let publisher = MessagePublisher::new(client.clone(), prefix.clone());

        info!("Starting outbound processor for prefix: {}", prefix);

        let (
            r1, r2, r3, r4, r5, r6, r7, r8, r9, r10,
            r11, r12, r13, r14, r15, r16, r17, r18, r19, r20,
            r21, r22, r23, r24, r25, r26, r27,
        ) = tokio::join!(
            Self::handle_send_messages(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_edit_messages(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_delete_messages(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_interaction_respond(http.clone(), client.clone(), prefix.clone()),
            Self::handle_interaction_defer(http.clone(), client.clone(), prefix.clone()),
            Self::handle_interaction_followup(
                http.clone(),
                client.clone(),
                prefix.clone(),
                streaming_messages.clone(),
            ),
            Self::handle_reaction_add(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_reaction_remove(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_typing(http.clone(), client.clone(), prefix.clone()),
            crate::outbound_streaming::handle_stream_messages(
                http.clone(),
                client.clone(),
                prefix.clone(),
                streaming_messages,
            ),
            Self::handle_modal_respond(http.clone(), client.clone(), prefix.clone()),
            Self::handle_autocomplete_respond(http.clone(), client.clone(), prefix.clone()),
            Self::handle_ban(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_kick(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_timeout(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_create_channel(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_edit_channel(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_delete_channel(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_create_role(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_assign_role(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_remove_role(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_delete_role(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_pin(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_unpin(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_bulk_delete(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_create_thread(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_archive_thread(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
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
            ("modal_respond", r11),
            ("autocomplete_respond", r12),
            ("ban", r13),
            ("kick", r14),
            ("timeout", r15),
            ("create_channel", r16),
            ("edit_channel", r17),
            ("delete_channel", r18),
            ("create_role", r19),
            ("assign_role", r20),
            ("remove_role", r21),
            ("delete_role", r22),
            ("pin", r23),
            ("unpin", r24),
            ("bulk_delete", r25),
            ("create_thread", r26),
            ("archive_thread", r27),
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
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_send(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<SendMessageCommand>(&subject).await?;

        info!("Listening for send_message commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let context = format!("Failed to send message to channel {}", cmd.channel_id);
                    let mut builder = CreateMessage::new().content(truncate(&cmd.content));
                    if let Some(reply_id) = cmd.reply_to_message_id {
                        builder = builder.reference_message((channel, MessageId::new(reply_id)));
                    }
                    for attempt in 0..=MAX_RETRIES {
                        match channel.send_message(&*http, builder.clone()).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
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
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_edit(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<EditMessageCommand>(&subject).await?;

        info!("Listening for edit_message commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_id = MessageId::new(cmd.message_id);
                    let context = format!(
                        "Failed to edit message {} in channel {}",
                        cmd.message_id, cmd.channel_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        let mut builder = EditMessage::new();
                        if let Some(content) = &cmd.content {
                            builder = builder.content(truncate(content));
                        }
                        match channel.edit_message(&*http, message_id, builder).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
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
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_delete(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteMessageCommand>(&subject)
            .await?;

        info!("Listening for delete_message commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_id = MessageId::new(cmd.message_id);
                    let context = format!(
                        "Failed to delete message {} in channel {}",
                        cmd.message_id, cmd.channel_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match channel.delete_message(&*http, message_id).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
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
                        msg = msg.content(truncate(content));
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
                        log_error(
                            &subject,
                            &format!("Failed to respond to interaction {}", cmd.interaction_id),
                            &e,
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
                        log_error(
                            &subject,
                            &format!("Failed to defer interaction {}", cmd.interaction_id),
                            &e,
                        );
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
        streaming_messages: StreamingMessages,
    ) -> Result<()> {
        use tokio::time::Instant;

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
                        builder = builder.content(truncate(content));
                    }
                    if cmd.ephemeral {
                        builder = builder.ephemeral(true);
                    }

                    match http
                        .create_followup_message(&cmd.interaction_token, &builder, Vec::new())
                        .await
                    {
                        Ok(msg) => {
                            // If a session_id was provided, register the message so that
                            // subsequent StreamMessageCommands can edit it progressively.
                            if let Some(session_id) = cmd.session_id {
                                streaming_messages.write().await.insert(
                                    session_id.clone(),
                                    StreamingState {
                                        channel_id: msg.channel_id.get(),
                                        message_id: msg.id.get(),
                                        last_edit: Instant::now(),
                                        edit_count: 0,
                                    },
                                );
                                info!(
                                    "Registered followup message {} for streaming session {}",
                                    msg.id, session_id
                                );
                            }
                        }
                        Err(e) => {
                            log_error(
                                &subject,
                                &format!(
                                    "Failed to send followup for token {}",
                                    cmd.interaction_token
                                ),
                                &e,
                            );
                        }
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
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::reaction_add(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<AddReactionCommand>(&subject).await?;

        info!("Listening for reaction_add commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_id = MessageId::new(cmd.message_id);
                    let reaction = parse_reaction_type(&cmd.emoji);
                    let context = format!(
                        "Failed to add reaction '{}' to message {}",
                        cmd.emoji, cmd.message_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match http.create_reaction(channel, message_id, &reaction).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
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
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::reaction_remove(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
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
                    let context = format!(
                        "Failed to remove reaction '{}' from message {}",
                        cmd.emoji, cmd.message_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match http
                            .delete_reaction_me(channel, message_id, &reaction)
                            .await
                        {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
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
                        log_error(
                            &subject,
                            &format!("Failed to broadcast typing in channel {}", cmd.channel_id),
                            &e,
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

    async fn handle_modal_respond(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        use serenity::builder::{CreateActionRow, CreateInputText};
        use serenity::all::InputTextStyle;

        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::interaction_modal_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<ModalRespondCommand>(&subject)
            .await?;

        info!("Listening for modal_respond commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let components: Vec<CreateActionRow> = cmd
                        .inputs
                        .iter()
                        .map(|inp| {
                            CreateActionRow::InputText(
                                CreateInputText::new(
                                    InputTextStyle::Short,
                                    &inp.custom_id,
                                    &inp.custom_id,
                                )
                                .value(&inp.value),
                            )
                        })
                        .collect();

                    use serenity::builder::CreateModal;
                    let modal = CreateModal::new(&cmd.custom_id, &cmd.title)
                        .components(components);
                    let response = CreateInteractionResponse::Modal(modal);

                    if let Err(e) = http
                        .create_interaction_response(
                            cmd.interaction_id.into(),
                            &cmd.interaction_token,
                            &response,
                            Vec::new(),
                        )
                        .await
                    {
                        log_error(
                            &subject,
                            &format!("Failed to respond with modal to {}", cmd.interaction_id),
                            &e,
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize modal_respond command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_autocomplete_respond(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::interaction_autocomplete_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<AutocompleteRespondCommand>(&subject)
            .await?;

        info!("Listening for autocomplete_respond commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let mut response = CreateAutocompleteResponse::new();
                    for choice in &cmd.choices {
                        response = response.add_string_choice(&choice.name, &choice.value);
                    }
                    let ir = CreateInteractionResponse::Autocomplete(response);

                    if let Err(e) = http
                        .create_interaction_response(
                            cmd.interaction_id.into(),
                            &cmd.interaction_token,
                            &ir,
                            Vec::new(),
                        )
                        .await
                    {
                        log_error(
                            &subject,
                            &format!("Failed autocomplete_respond for {}", cmd.interaction_id),
                            &e,
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize autocomplete_respond command: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_ban(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::guild_ban(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<BanUserCommand>(&subject).await?;

        info!("Listening for ban commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let user = UserId::new(cmd.user_id);
                    let context = format!("Failed to ban user {} in guild {}", cmd.user_id, cmd.guild_id);
                    // dmd = delete message days (max 7), API expects u8
                    let delete_days = (cmd.delete_message_seconds / 86400).min(7) as u8;
                    for attempt in 0..=MAX_RETRIES {
                        match guild.ban(http.as_ref(), user, delete_days).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize ban command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_kick(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::guild_kick(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<KickUserCommand>(&subject).await?;

        info!("Listening for kick commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let user = UserId::new(cmd.user_id);
                    let context = format!("Failed to kick user {} from guild {}", cmd.user_id, cmd.guild_id);
                    for attempt in 0..=MAX_RETRIES {
                        match guild.kick(http.as_ref(), user).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize kick command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_timeout(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::guild_timeout(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<TimeoutUserCommand>(&subject).await?;

        info!("Listening for timeout commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let user = UserId::new(cmd.user_id);
                    let context = format!("Failed to timeout user {} in guild {}", cmd.user_id, cmd.guild_id);

                    let edit = if cmd.duration_secs == 0 {
                        EditMember::new().enable_communication()
                    } else {
                        let now_secs = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let until_secs = (now_secs + cmd.duration_secs) as i64;
                        let timestamp =
                            serenity::model::Timestamp::from_unix_timestamp(until_secs)
                                .unwrap_or_else(|_| serenity::model::Timestamp::now());
                        EditMember::new().disable_communication_until_datetime(timestamp)
                    };

                    for attempt in 0..=MAX_RETRIES {
                        match guild.edit_member(http.as_ref(), user, edit.clone()).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize timeout command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_create_channel(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        use discord_types::types::ChannelType as DcChannelType;
        use serenity::model::channel::ChannelType as SerenityChannelType;

        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::channel_create(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<CreateChannelCommand>(&subject).await?;

        info!("Listening for create_channel commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let context = format!("Failed to create channel '{}' in guild {}", cmd.name, cmd.guild_id);
                    let kind = match cmd.channel_type {
                        DcChannelType::GuildVoice => SerenityChannelType::Voice,
                        DcChannelType::GuildCategory => SerenityChannelType::Category,
                        DcChannelType::GuildNews => SerenityChannelType::News,
                        DcChannelType::GuildStageVoice => SerenityChannelType::Stage,
                        DcChannelType::GuildForum => SerenityChannelType::Forum,
                        _ => SerenityChannelType::Text,
                    };
                    let mut builder = CreateChannel::new(&cmd.name).kind(kind);
                    if let Some(topic) = cmd.topic {
                        builder = builder.topic(&topic);
                    }
                    if let Some(cat_id) = cmd.category_id {
                        builder = builder.category(ChannelId::new(cat_id));
                    }
                    for attempt in 0..=MAX_RETRIES {
                        match guild.create_channel(http.as_ref(), builder.clone()).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize create_channel command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_edit_channel(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::channel_edit(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<EditChannelCommand>(&subject).await?;

        info!("Listening for edit_channel commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let context = format!("Failed to edit channel {}", cmd.channel_id);
                    let mut builder = EditChannel::new();
                    if let Some(ref name) = cmd.name {
                        builder = builder.name(name);
                    }
                    if let Some(ref topic) = cmd.topic {
                        builder = builder.topic(topic);
                    }
                    for attempt in 0..=MAX_RETRIES {
                        match channel.edit(http.as_ref(), builder.clone()).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize edit_channel command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_delete_channel(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::channel_delete(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<DeleteChannelCommand>(&subject).await?;

        info!("Listening for delete_channel commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let context = format!("Failed to delete channel {}", cmd.channel_id);
                    for attempt in 0..=MAX_RETRIES {
                        match channel.delete(http.as_ref()).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize delete_channel command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_create_role(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::role_create(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<CreateRoleCommand>(&subject).await?;

        info!("Listening for create_role commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let context = format!("Failed to create role '{}' in guild {}", cmd.name, cmd.guild_id);
                    let mut builder = EditRole::new()
                        .name(&cmd.name)
                        .hoist(cmd.hoist)
                        .mentionable(cmd.mentionable);
                    if let Some(color) = cmd.color {
                        builder = builder.colour(color);
                    }
                    for attempt in 0..=MAX_RETRIES {
                        match guild.create_role(http.as_ref(), builder.clone()).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize create_role command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_assign_role(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::role_assign(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<AssignRoleCommand>(&subject).await?;

        info!("Listening for assign_role commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let context = format!("Failed to assign role {} to user {} in guild {}", cmd.role_id, cmd.user_id, cmd.guild_id);
                    for attempt in 0..=MAX_RETRIES {
                        match http
                            .add_member_role(
                                GuildId::new(cmd.guild_id),
                                UserId::new(cmd.user_id),
                                RoleId::new(cmd.role_id),
                                None,
                            )
                            .await
                        {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize assign_role command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_remove_role(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::role_remove(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<RemoveRoleCommand>(&subject).await?;

        info!("Listening for remove_role commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let context = format!("Failed to remove role {} from user {} in guild {}", cmd.role_id, cmd.user_id, cmd.guild_id);
                    for attempt in 0..=MAX_RETRIES {
                        match http
                            .remove_member_role(
                                GuildId::new(cmd.guild_id),
                                UserId::new(cmd.user_id),
                                RoleId::new(cmd.role_id),
                                None,
                            )
                            .await
                        {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize remove_role command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_delete_role(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::role_delete(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<DeleteRoleCommand>(&subject).await?;

        info!("Listening for delete_role commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let role = RoleId::new(cmd.role_id);
                    let context = format!("Failed to delete role {} in guild {}", cmd.role_id, cmd.guild_id);
                    for attempt in 0..=MAX_RETRIES {
                        match guild.delete_role(http.as_ref(), role).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize delete_role command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_pin(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_pin(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<PinMessageCommand>(&subject).await?;

        info!("Listening for pin commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message = MessageId::new(cmd.message_id);
                    let context = format!("Failed to pin message {} in channel {}", cmd.message_id, cmd.channel_id);
                    for attempt in 0..=MAX_RETRIES {
                        match channel.pin(http.as_ref(), message).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize pin command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_unpin(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_unpin(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<UnpinMessageCommand>(&subject).await?;

        info!("Listening for unpin commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message = MessageId::new(cmd.message_id);
                    let context = format!("Failed to unpin message {} in channel {}", cmd.message_id, cmd.channel_id);
                    for attempt in 0..=MAX_RETRIES {
                        match channel.unpin(http.as_ref(), message).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize unpin command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_bulk_delete(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_bulk_delete(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber
            .subscribe::<BulkDeleteMessagesCommand>(&subject)
            .await?;

        info!("Listening for bulk_delete commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message_ids: Vec<MessageId> = cmd
                        .message_ids
                        .iter()
                        .map(|&id| MessageId::new(id))
                        .collect();
                    let context = format!("Failed to bulk delete {} messages in channel {}", message_ids.len(), cmd.channel_id);
                    for attempt in 0..=MAX_RETRIES {
                        match channel.delete_messages(http.as_ref(), &message_ids).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize bulk_delete command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_create_thread(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        use serenity::model::channel::AutoArchiveDuration;

        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::thread_create(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<CreateThreadCommand>(&subject).await?;

        info!("Listening for create_thread commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let context = format!("Failed to create thread '{}' in channel {}", cmd.name, cmd.channel_id);
                    let archive_dur = match cmd.auto_archive_mins {
                        60 => AutoArchiveDuration::OneHour,
                        4320 => AutoArchiveDuration::ThreeDays,
                        10080 => AutoArchiveDuration::OneWeek,
                        _ => AutoArchiveDuration::OneDay,
                    };
                    let builder = CreateThread::new(&cmd.name)
                        .auto_archive_duration(archive_dur);
                    for attempt in 0..=MAX_RETRIES {
                        let result = if let Some(msg_id) = cmd.message_id {
                            channel
                                .create_thread_from_message(
                                    http.as_ref(),
                                    MessageId::new(msg_id),
                                    builder.clone(),
                                )
                                .await
                        } else {
                            channel.create_thread(http.as_ref(), builder.clone()).await
                        };
                        match result {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize create_thread command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_archive_thread(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::thread_archive(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber
            .subscribe::<ArchiveThreadCommand>(&subject)
            .await?;

        info!("Listening for archive_thread commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let context = format!("Failed to archive thread {}", cmd.channel_id);
                    let builder = EditThread::new().archived(true).locked(cmd.locked);
                    for attempt in 0..=MAX_RETRIES {
                        match channel.edit_thread(http.as_ref(), builder.clone()).await {
                            Ok(_) => break,
                            Err(e) => match classify(&subject, &e) {
                                ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                    tokio::time::sleep(dur).await;
                                }
                                outcome => {
                                    publish_if_permanent(&publisher, &error_subject, &outcome).await;
                                    log_outcome(&subject, &context, outcome);
                                    break;
                                }
                            },
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize archive_thread command: {}", e),
            }
        }

        Ok(())
    }
}

/// Publish a `CommandErrorEvent` to NATS if the outcome is `Permanent`.
///
/// Failures to publish are only logged as warnings so they never block the
/// main error-handling path.
async fn publish_if_permanent(
    publisher: &MessagePublisher,
    error_subject: &str,
    outcome: &ErrorOutcome,
) {
    if let ErrorOutcome::Permanent(evt) = outcome {
        if let Err(e) = publisher.publish(error_subject, evt).await {
            warn!("Failed to publish command_error to NATS: {}", e);
        }
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
