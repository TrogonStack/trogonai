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
    BanUserCommand, BulkDeleteMessagesCommand, CreateChannelCommand, CreateEmojiCommand,
    CreateInviteCommand, CreateRoleCommand, CreateScheduledEventCommand, CreateThreadCommand,
    CreateWebhookCommand, DeleteChannelCommand, DeleteEmojiCommand, DeleteMessageCommand,
    DeleteRoleCommand, DeleteScheduledEventCommand, DeleteWebhookCommand, EditChannelCommand,
    EditMessageCommand, ExecuteWebhookCommand, FetchMemberCommand, FetchMessagesCommand,
    GuildMemberNickCommand, InteractionDeferCommand, InteractionFollowupCommand,
    InteractionRespondCommand, KickUserCommand, ModalRespondCommand, PinMessageCommand,
    RemoveReactionCommand, RemoveRoleCommand, RevokeInviteCommand, SendMessageCommand,
    SetBotPresenceCommand, TimeoutUserCommand, TypingCommand, UnbanUserCommand,
    UnpinMessageCommand, VoiceDisconnectCommand, VoiceMoveCommand,
};
use discord_types::types::{
    ActionRowComponent, AttachedFile, ButtonStyle as DcButtonStyle, DiscordUser,
    Embed, EmbedAuthor, EmbedField, EmbedFooter, EmbedMedia, FetchedMember, FetchedMessage,
};
use serenity::builder::{
    CreateActionRow, CreateAttachment, CreateAutocompleteResponse, CreateButton, CreateChannel,
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseFollowup, CreateInteractionResponseMessage, CreateMessage,
    CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, CreateThread, EditChannel,
    EditMember, EditMessage, EditRole, EditThread, GetMessages,
};
use serenity::model::application::ButtonStyle as SerenityButtonStyle;
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

/// Convert a discord-types `Embed` into a serenity `CreateEmbed`.
fn build_embed(embed: &Embed) -> CreateEmbed {
    let mut builder = CreateEmbed::new();
    if let Some(ref title) = embed.title {
        builder = builder.title(title);
    }
    if let Some(ref desc) = embed.description {
        builder = builder.description(desc);
    }
    if let Some(ref url) = embed.url {
        builder = builder.url(url);
    }
    if let Some(color) = embed.color {
        builder = builder.color(color);
    }
    for field in &embed.fields {
        builder = builder.field(&field.name, &field.value, field.inline);
    }
    if let Some(ref author) = embed.author {
        let mut a = CreateEmbedAuthor::new(&author.name);
        if let Some(ref url) = author.url {
            a = a.url(url);
        }
        if let Some(ref icon) = author.icon_url {
            a = a.icon_url(icon);
        }
        builder = builder.author(a);
    }
    if let Some(ref footer) = embed.footer {
        let mut f = CreateEmbedFooter::new(&footer.text);
        if let Some(ref icon) = footer.icon_url {
            f = f.icon_url(icon);
        }
        builder = builder.footer(f);
    }
    if let Some(ref image) = embed.image {
        builder = builder.image(&image.url);
    }
    if let Some(ref thumbnail) = embed.thumbnail {
        builder = builder.thumbnail(&thumbnail.url);
    }
    if let Some(ref ts) = embed.timestamp {
        if let Ok(timestamp) = serenity::model::Timestamp::parse(ts) {
            builder = builder.timestamp(timestamp);
        }
    }
    builder
}

/// Convert discord-types `ActionRow` slice into serenity `CreateActionRow` vec.
fn build_action_rows(rows: &[discord_types::types::ActionRow]) -> Vec<CreateActionRow> {
    rows.iter()
        .filter_map(|row| {
            let has_buttons = row
                .components
                .iter()
                .any(|c| matches!(c, ActionRowComponent::Button(_)));
            let has_select = row
                .components
                .iter()
                .any(|c| matches!(
                    c,
                    ActionRowComponent::StringSelect(_)
                        | ActionRowComponent::UserSelect(_)
                        | ActionRowComponent::RoleSelect(_)
                        | ActionRowComponent::ChannelSelect(_)
                ));

            if has_buttons && !has_select {
                let buttons: Vec<CreateButton> = row
                    .components
                    .iter()
                    .filter_map(|c| {
                        if let ActionRowComponent::Button(btn) = c {
                            let mut b = match btn.style {
                                DcButtonStyle::Link => {
                                    CreateButton::new_link(
                                        btn.url.as_deref().unwrap_or(""),
                                    )
                                }
                                _ => {
                                    let style = match btn.style {
                                        DcButtonStyle::Secondary => {
                                            SerenityButtonStyle::Secondary
                                        }
                                        DcButtonStyle::Success => SerenityButtonStyle::Success,
                                        DcButtonStyle::Danger => SerenityButtonStyle::Danger,
                                        _ => SerenityButtonStyle::Primary,
                                    };
                                    CreateButton::new(
                                        btn.custom_id.as_deref().unwrap_or(""),
                                    )
                                    .style(style)
                                }
                            };
                            if let Some(ref label) = btn.label {
                                b = b.label(label);
                            }
                            if btn.disabled {
                                b = b.disabled(true);
                            }
                            Some(b)
                        } else {
                            None
                        }
                    })
                    .collect();
                Some(CreateActionRow::Buttons(buttons))
            } else if has_select {
                let (menu, kind) = row.components.iter().find_map(|c| match c {
                    ActionRowComponent::StringSelect(m) => {
                        let opts: Vec<CreateSelectMenuOption> = m
                            .options
                            .iter()
                            .map(|opt| {
                                let mut o =
                                    CreateSelectMenuOption::new(&opt.label, &opt.value);
                                if let Some(ref desc) = opt.description {
                                    o = o.description(desc);
                                }
                                if opt.default {
                                    o = o.default_selection(true);
                                }
                                o
                            })
                            .collect();
                        Some((m, CreateSelectMenuKind::String { options: opts }))
                    }
                    ActionRowComponent::UserSelect(m) => {
                        Some((m, CreateSelectMenuKind::User { default_users: None }))
                    }
                    ActionRowComponent::RoleSelect(m) => {
                        Some((m, CreateSelectMenuKind::Role { default_roles: None }))
                    }
                    ActionRowComponent::ChannelSelect(m) => Some((
                        m,
                        CreateSelectMenuKind::Channel {
                            channel_types: None,
                            default_channels: None,
                        },
                    )),
                    _ => None,
                })?;
                let mut select = CreateSelectMenu::new(&menu.custom_id, kind)
                .min_values(menu.min_values)
                .max_values(menu.max_values)
                .disabled(menu.disabled);
                if let Some(ref placeholder) = menu.placeholder {
                    select = select.placeholder(placeholder);
                }
                Some(CreateActionRow::SelectMenu(select))
            } else {
                None
            }
        })
        .collect()
}

/// Load file attachments from local paths into serenity `CreateAttachment` values.
async fn load_attachments(files: &[AttachedFile]) -> Vec<CreateAttachment> {
    let mut result = Vec::with_capacity(files.len());
    for file in files {
        match CreateAttachment::path(&file.path).await {
            Ok(mut a) => {
                if let Some(ref desc) = file.description {
                    a = a.description(desc);
                }
                result.push(a);
            }
            Err(e) => {
                error!("Failed to load attachment '{}': {}", file.path, e);
            }
        }
    }
    result
}

/// Convert a serenity `Message` to a discord-types `FetchedMessage`.
fn to_fetched_message(m: &serenity::model::channel::Message) -> FetchedMessage {
    FetchedMessage {
        id: m.id.get(),
        channel_id: m.channel_id.get(),
        author: DiscordUser {
            id: m.author.id.get(),
            username: m.author.name.clone(),
            global_name: m.author.global_name.as_deref().map(String::from),
            bot: m.author.bot,
        },
        content: m.content.clone(),
        timestamp: m.timestamp.to_rfc3339().unwrap_or_default(),
        attachments: m
            .attachments
            .iter()
            .map(|a| discord_types::types::Attachment {
                id: a.id.get(),
                filename: a.filename.clone(),
                url: a.url.clone(),
                content_type: a.content_type.clone(),
                size: a.size as u64,
            })
            .collect(),
        embeds: m
            .embeds
            .iter()
            .map(|e| Embed {
                title: e.title.clone(),
                description: e.description.clone(),
                url: e.url.clone(),
                fields: e
                    .fields
                    .iter()
                    .map(|f| EmbedField {
                        name: f.name.clone(),
                        value: f.value.clone(),
                        inline: f.inline,
                    })
                    .collect(),
                color: e.colour.map(|c| c.0),
                author: e.author.as_ref().map(|a| EmbedAuthor {
                    name: a.name.clone(),
                    url: a.url.clone(),
                    icon_url: a.icon_url.clone(),
                }),
                footer: e.footer.as_ref().map(|f| EmbedFooter {
                    text: f.text.clone(),
                    icon_url: f.icon_url.clone(),
                }),
                image: e.image.as_ref().map(|i| EmbedMedia { url: i.url.clone() }),
                thumbnail: e.thumbnail.as_ref().map(|t| EmbedMedia { url: t.url.clone() }),
                timestamp: e.timestamp.map(|t| t.to_string()),
            })
            .collect(),
    }
}

/// Processes NATS agent commands and forwards them to Discord
pub struct OutboundProcessor {
    pub(crate) http: Arc<Http>,
    pub(crate) client: async_nats::Client,
    pub(crate) prefix: String,
    pub(crate) streaming_messages: StreamingMessages,
    pub(crate) shard_manager: Option<Arc<serenity::all::ShardManager>>,
}

impl OutboundProcessor {
    pub fn new(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        shard_manager: Option<Arc<serenity::all::ShardManager>>,
    ) -> Self {
        Self {
            http,
            client,
            prefix,
            streaming_messages: Arc::new(RwLock::new(HashMap::new())),
            shard_manager,
        }
    }

    /// Start all command handler tasks
    pub async fn run(self) -> Result<()> {
        let http = self.http;
        let client = self.client;
        let prefix = self.prefix;
        let streaming_messages = self.streaming_messages;
        let shard_manager = self.shard_manager;
        let publisher = MessagePublisher::new(client.clone(), prefix.clone());

        info!("Starting outbound processor for prefix: {}", prefix);

        let (
            r1, r2, r3, r4, r5, r6, r7, r8, r9, r10,
            r11, r12, r13, r14, r15, r16, r17, r18, r19, r20,
            r21, r22, r23, r24, r25, r26, r27, r28, r29, r30,
            r31, r32, r33, r34, r35, r36, r37,
            r38, r39, r40, r41, r42, r43,
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
            Self::handle_create_channel(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_channel(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_delete_channel(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_create_role(http.clone(), client.clone(), prefix.clone()),
            Self::handle_assign_role(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_remove_role(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_delete_role(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_pin(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_unpin(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_bulk_delete(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_create_thread(http.clone(), client.clone(), prefix.clone()),
            Self::handle_archive_thread(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_bot_presence(client.clone(), prefix.clone(), shard_manager),
            Self::handle_fetch_messages(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_member(http.clone(), client.clone(), prefix.clone()),
            Self::handle_unban(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_guild_member_nick(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_webhook_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_webhook_execute(http.clone(), client.clone(), prefix.clone()),
            Self::handle_webhook_delete(http.clone(), client.clone(), prefix.clone()),
            Self::handle_voice_move(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_voice_disconnect(http.clone(), client.clone(), prefix.clone(), publisher.clone()),
            Self::handle_invite_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_invite_revoke(http.clone(), client.clone(), prefix.clone()),
            Self::handle_emoji_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_emoji_delete(http.clone(), client.clone(), prefix.clone()),
            Self::handle_scheduled_event_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_scheduled_event_delete(http.clone(), client.clone(), prefix.clone()),
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
            ("bot_presence", r28),
            ("fetch_messages", r29),
            ("fetch_member", r30),
            ("unban", r31),
            ("guild_member_nick", r32),
            ("webhook_create", r33),
            ("webhook_execute", r34),
            ("webhook_delete", r35),
            ("voice_move", r36),
            ("voice_disconnect", r37),
            ("invite_create", r38),
            ("invite_revoke", r39),
            ("emoji_create", r40),
            ("emoji_delete", r41),
            ("scheduled_event_create", r42),
            ("scheduled_event_delete", r43),
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
                    // Load attachments before the retry loop (async path reads)
                    let attachments = load_attachments(&cmd.files).await;
                    let mut builder = CreateMessage::new().content(truncate(&cmd.content));
                    if let Some(reply_id) = cmd.reply_to_message_id {
                        builder = builder.reference_message((channel, MessageId::new(reply_id)));
                    }
                    if !cmd.embeds.is_empty() {
                        let embeds: Vec<CreateEmbed> = cmd.embeds.iter().map(build_embed).collect();
                        builder = builder.embeds(embeds);
                    }
                    if !cmd.components.is_empty() {
                        builder = builder.components(build_action_rows(&cmd.components));
                    }
                    builder = builder.add_files(attachments);
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
                        if !cmd.embeds.is_empty() {
                            let embeds: Vec<CreateEmbed> = cmd.embeds.iter().map(build_embed).collect();
                            builder = builder.embeds(embeds);
                        }
                        if !cmd.components.is_empty() {
                            builder = builder.components(build_action_rows(&cmd.components));
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
                    if !cmd.embeds.is_empty() {
                        let embeds: Vec<CreateEmbed> = cmd.embeds.iter().map(build_embed).collect();
                        msg = msg.embeds(embeds);
                    }
                    if !cmd.components.is_empty() {
                        msg = msg.components(build_action_rows(&cmd.components));
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
                    let attachments = load_attachments(&cmd.files).await;
                    let mut builder = CreateInteractionResponseFollowup::new();
                    if let Some(content) = &cmd.content {
                        builder = builder.content(truncate(content));
                    }
                    if cmd.ephemeral {
                        builder = builder.ephemeral(true);
                    }
                    if !cmd.embeds.is_empty() {
                        let embeds: Vec<CreateEmbed> = cmd.embeds.iter().map(build_embed).collect();
                        builder = builder.embeds(embeds);
                    }
                    if !cmd.components.is_empty() {
                        builder = builder.components(build_action_rows(&cmd.components));
                    }

                    match http
                        .create_followup_message(&cmd.interaction_token, &builder, attachments)
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
                    let reason = cmd.reason.as_deref().unwrap_or("No reason provided");
                    for attempt in 0..=MAX_RETRIES {
                        match guild.ban_with_reason(http.as_ref(), user, delete_days, reason).await {
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
                    let reason = cmd.reason.as_deref().unwrap_or("");
                    let context = format!("Failed to kick user {} from guild {}", cmd.user_id, cmd.guild_id);
                    for attempt in 0..=MAX_RETRIES {
                        match guild.kick_with_reason(http.as_ref(), user, reason).await {
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
    ) -> Result<()> {
        use discord_types::types::ChannelType as DcChannelType;
        use serenity::futures::StreamExt as _;
        use serenity::model::channel::ChannelType as SerenityChannelType;

        let subject = subjects::agent::channel_create(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;

        info!("Listening for create_channel commands on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: CreateChannelCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to deserialize create_channel command: {}", e);
                    continue;
                }
            };

            let reply_inbox = msg.reply.clone();
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
                    Ok(created_channel) => {
                        if let Some(reply) = reply_inbox {
                            let _ = client
                                .publish(reply, created_channel.id.get().to_string().into())
                                .await;
                        }
                        break;
                    }
                    Err(e) => match classify(&subject, &e) {
                        ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                            tokio::time::sleep(dur).await;
                        }
                        outcome => {
                            log_outcome(&subject, &context, outcome);
                            break;
                        }
                    },
                }
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
    ) -> Result<()> {
        use serenity::futures::StreamExt as _;

        let subject = subjects::agent::role_create(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;

        info!("Listening for create_role commands on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: CreateRoleCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to deserialize create_role command: {}", e);
                    continue;
                }
            };

            let reply_inbox = msg.reply.clone();
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
                    Ok(created_role) => {
                        if let Some(reply) = reply_inbox {
                            let _ = client
                                .publish(reply, created_role.id.get().to_string().into())
                                .await;
                        }
                        break;
                    }
                    Err(e) => match classify(&subject, &e) {
                        ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                            tokio::time::sleep(dur).await;
                        }
                        outcome => {
                            log_outcome(&subject, &context, outcome);
                            break;
                        }
                    },
                }
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
    ) -> Result<()> {
        use serenity::futures::StreamExt as _;
        use serenity::model::channel::AutoArchiveDuration;

        let subject = subjects::agent::thread_create(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;

        info!("Listening for create_thread commands on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: CreateThreadCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to deserialize create_thread command: {}", e);
                    continue;
                }
            };

            let reply_inbox = msg.reply.clone();
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
                let thread_result = if let Some(msg_id) = cmd.message_id {
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
                match thread_result {
                    Ok(created_thread) => {
                        if let Some(reply) = reply_inbox {
                            let _ = client
                                .publish(reply, created_thread.id.get().to_string().into())
                                .await;
                        }
                        break;
                    }
                    Err(e) => match classify(&subject, &e) {
                        ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                            tokio::time::sleep(dur).await;
                        }
                        outcome => {
                            log_outcome(&subject, &context, outcome);
                            break;
                        }
                    },
                }
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

    /// Handle `SetBotPresenceCommand`.
    ///
    /// When `shard_manager` is provided, broadcasts the new presence to all shards.
    /// When it is `None` (e.g. in tests), logs a warning and skips the update.
    async fn handle_bot_presence(
        client: async_nats::Client,
        prefix: String,
        shard_manager: Option<Arc<serenity::all::ShardManager>>,
    ) -> Result<()> {
        use discord_types::types::{ActivityType, BotStatus};
        use serenity::futures::StreamExt as _;
        use serenity::gateway::ActivityData;
        use serenity::model::user::OnlineStatus;

        let subject = subjects::agent::bot_presence(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;

        info!("Listening for bot_presence commands on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: SetBotPresenceCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to deserialize bot_presence command: {}", e);
                    continue;
                }
            };

            let Some(ref sm) = shard_manager else {
                warn!(
                    "bot_presence command received but shard_manager is not available; \
                     skipping presence update."
                );
                continue;
            };

            let online_status = match cmd.status {
                BotStatus::Online => OnlineStatus::Online,
                BotStatus::Idle => OnlineStatus::Idle,
                BotStatus::DoNotDisturb => OnlineStatus::DoNotDisturb,
                BotStatus::Invisible => OnlineStatus::Invisible,
            };

            let activity = cmd.activity.map(|a| match a.kind {
                ActivityType::Playing => ActivityData::playing(a.name),
                ActivityType::Listening => ActivityData::listening(a.name),
                ActivityType::Watching => ActivityData::watching(a.name),
                ActivityType::Competing => ActivityData::competing(a.name),
                ActivityType::Streaming => {
                    if let Some(url) = a.url {
                        ActivityData::streaming(a.name.clone(), url).unwrap_or_else(|_| ActivityData::playing(a.name))
                    } else {
                        ActivityData::playing(a.name)
                    }
                }
                ActivityType::Custom => ActivityData::playing(a.name),
            });

            let runners = sm.runners.lock().await;
            for info in runners.values() {
                info.runner_tx.set_presence(activity.clone(), online_status);
            }
        }

        Ok(())
    }

    /// Handle `FetchMessagesCommand` (request-reply pattern).
    ///
    /// The agent sends a `FetchMessagesCommand` via `client.request()`. This
    /// handler fetches the messages from Discord and publishes the result as
    /// `Vec<FetchedMessage>` JSON to the NATS reply inbox.
    async fn handle_fetch_messages(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_messages(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;

        info!("Listening for fetch_messages requests on {}", subject);

        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_messages: received message without reply subject");
                    continue;
                }
            };

            let cmd: FetchMessagesCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_messages: failed to deserialize: {}", e);
                    continue;
                }
            };

            let channel = ChannelId::new(cmd.channel_id);
            let limit = cmd.limit.clamp(1, 100);
            let mut get_messages = GetMessages::new().limit(limit);
            if let Some(before_id) = cmd.before_id {
                get_messages = get_messages.before(MessageId::new(before_id));
            }

            let response_bytes = match channel.messages(http.as_ref(), get_messages).await {
                Ok(messages) => {
                    let fetched: Vec<FetchedMessage> =
                        messages.iter().map(to_fetched_message).collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_messages: Discord API error for channel {}: {}",
                        cmd.channel_id, e
                    );
                    serde_json::to_vec::<Vec<FetchedMessage>>(&vec![]).unwrap_or_default()
                }
            };

            if let Err(e) = client.publish(reply, response_bytes.into()).await {
                error!("fetch_messages: failed to send reply: {}", e);
            }
        }

        Ok(())
    }

    /// Handle `FetchMemberCommand` (request-reply pattern).
    ///
    /// The agent sends a `FetchMemberCommand` via `client.request()`. This
    /// handler fetches the guild member from Discord and publishes the result as
    /// `Option<FetchedMember>` JSON to the NATS reply inbox (`null` on error).
    async fn handle_fetch_member(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_member(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;

        info!("Listening for fetch_member requests on {}", subject);

        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_member: received message without reply subject");
                    continue;
                }
            };

            let cmd: FetchMemberCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_member: failed to deserialize: {}", e);
                    continue;
                }
            };

            let guild = GuildId::new(cmd.guild_id);
            let user = UserId::new(cmd.user_id);

            let response_bytes =
                match guild.member(http.as_ref(), user).await {
                    Ok(member) => {
                        let fetched = FetchedMember {
                            user: DiscordUser {
                                id: member.user.id.get(),
                                username: member.user.name.clone(),
                                global_name: member
                                    .user
                                    .global_name
                                    .as_deref()
                                    .map(String::from),
                                bot: member.user.bot,
                            },
                            guild_id: cmd.guild_id,
                            nick: member.nick.clone(),
                            roles: member.roles.iter().map(|r| r.get()).collect(),
                            joined_at: member.joined_at.and_then(|t| t.to_rfc3339()),
                        };
                        serde_json::to_vec(&fetched).unwrap_or_default()
                    }
                    Err(e) => {
                        error!(
                            "fetch_member: Discord API error for user {} in guild {}: {}",
                            cmd.user_id, cmd.guild_id, e
                        );
                        b"null".to_vec()
                    }
                };

            if let Err(e) = client.publish(reply, response_bytes.into()).await {
                error!("fetch_member: failed to send reply: {}", e);
            }
        }

        Ok(())
    }

    async fn handle_unban(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::guild_unban(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<UnbanUserCommand>(&subject).await?;

        info!("Listening for unban commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let user = UserId::new(cmd.user_id);
                    let context = format!("Failed to unban user {} in guild {}", cmd.user_id, cmd.guild_id);
                    for attempt in 0..=MAX_RETRIES {
                        match http.remove_ban(guild, user, None).await {
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
                Err(e) => warn!("Failed to deserialize unban command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_guild_member_nick(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::guild_member_nick(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<GuildMemberNickCommand>(&subject).await?;

        info!("Listening for guild_member_nick commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let user = UserId::new(cmd.user_id);
                    let context = format!(
                        "Failed to set nick for user {} in guild {}",
                        cmd.user_id, cmd.guild_id
                    );
                    let edit = match cmd.nick {
                        Some(ref nick) => EditMember::new().nickname(nick),
                        None => EditMember::new().nickname(""),
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
                Err(e) => warn!("Failed to deserialize guild_member_nick command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_webhook_create(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::webhook_create(&prefix);
        let mut stream = subscriber.subscribe::<CreateWebhookCommand>(&subject).await?;
        info!("Listening for webhook_create commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let builder = serenity::builder::CreateWebhook::new(&cmd.name);
                    if let Err(e) = channel.create_webhook(&*http, builder).await {
                        warn!("Failed to create webhook in {}: {}", cmd.channel_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize webhook_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_webhook_execute(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::webhook_execute(&prefix);
        let mut stream = subscriber.subscribe::<ExecuteWebhookCommand>(&subject).await?;
        info!("Listening for webhook_execute commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let mut builder = serenity::builder::ExecuteWebhook::new()
                        .content(cmd.content);
                    if let Some(ref name) = cmd.username {
                        builder = builder.username(name);
                    }
                    if let Some(ref url) = cmd.avatar_url {
                        builder = builder.avatar_url(url);
                    }
                    if let Err(e) = http
                        .execute_webhook(cmd.webhook_id.into(), None, &cmd.webhook_token, false, Vec::new(), &builder)
                        .await
                    {
                        warn!("Failed to execute webhook {}: {}", cmd.webhook_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize webhook_execute command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_webhook_delete(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::webhook_delete(&prefix);
        let mut stream = subscriber.subscribe::<DeleteWebhookCommand>(&subject).await?;
        info!("Listening for webhook_delete commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    if let Err(e) = http
                        .delete_webhook_with_token(cmd.webhook_id.into(), &cmd.webhook_token, None)
                        .await
                    {
                        warn!("Failed to delete webhook {}: {}", cmd.webhook_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize webhook_delete command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_voice_move(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::voice_move(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<VoiceMoveCommand>(&subject).await?;
        info!("Listening for voice_move commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let user = UserId::new(cmd.user_id);
                    let channel = ChannelId::new(cmd.channel_id);
                    let context = format!(
                        "Failed to move user {} to channel {} in guild {}",
                        cmd.user_id, cmd.channel_id, cmd.guild_id
                    );
                    let edit = EditMember::new().voice_channel(channel);
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
                Err(e) => warn!("Failed to deserialize voice_move command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_voice_disconnect(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::voice_disconnect(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber.subscribe::<VoiceDisconnectCommand>(&subject).await?;
        info!("Listening for voice_disconnect commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let user = UserId::new(cmd.user_id);
                    let context = format!(
                        "Failed to disconnect user {} from voice in guild {}",
                        cmd.user_id, cmd.guild_id
                    );
                    // Disconnect by editing member with no voice channel
                    let edit = EditMember::new().disconnect_member();
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
                Err(e) => warn!("Failed to deserialize voice_disconnect command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_invite_create(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::invite_create(&prefix);
        let mut stream = subscriber.subscribe::<CreateInviteCommand>(&subject).await?;
        info!("Listening for invite_create commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let builder = serenity::builder::CreateInvite::new()
                        .max_age(cmd.max_age_secs as u32)
                        .max_uses(cmd.max_uses as u8)
                        .temporary(cmd.temporary)
                        .unique(cmd.unique);
                    if let Err(e) = channel.create_invite(&*http, builder).await {
                        warn!("Failed to create invite for channel {}: {}", cmd.channel_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize invite_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_invite_revoke(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::invite_revoke(&prefix);
        let mut stream = subscriber.subscribe::<RevokeInviteCommand>(&subject).await?;
        info!("Listening for invite_revoke commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let reason = cmd.reason.as_deref();
                    if let Err(e) = http.delete_invite(&cmd.code, reason).await {
                        warn!("Failed to revoke invite {}: {}", cmd.code, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize invite_revoke command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_emoji_create(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::emoji_create(&prefix);
        let mut stream = subscriber.subscribe::<CreateEmojiCommand>(&subject).await?;
        info!("Listening for emoji_create commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    if let Err(e) = guild.create_emoji(&*http, &cmd.name, &cmd.image_data).await {
                        warn!("Failed to create emoji '{}' in guild {}: {}", cmd.name, cmd.guild_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize emoji_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_emoji_delete(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::emoji_delete(&prefix);
        let mut stream = subscriber.subscribe::<DeleteEmojiCommand>(&subject).await?;
        info!("Listening for emoji_delete commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let emoji_id = serenity::model::id::EmojiId::new(cmd.emoji_id);
                    if let Err(e) = guild.delete_emoji(&*http, emoji_id).await {
                        warn!("Failed to delete emoji {} from guild {}: {}", cmd.emoji_id, cmd.guild_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize emoji_delete command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_scheduled_event_create(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        use serenity::builder::CreateScheduledEvent;
        use serenity::model::guild::ScheduledEventType;

        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::scheduled_event_create(&prefix);
        let mut stream = subscriber.subscribe::<CreateScheduledEventCommand>(&subject).await?;
        info!("Listening for scheduled_event_create commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let start = match cmd.start_time.parse::<serenity::model::Timestamp>() {
                        Ok(t) => t,
                        Err(e) => {
                            warn!("Invalid start_time '{}': {}", cmd.start_time, e);
                            continue;
                        }
                    };

                    let (kind, mut builder) = if let Some(channel_id) = cmd.channel_id {
                        let b = CreateScheduledEvent::new(ScheduledEventType::Voice, &cmd.name, start)
                            .channel_id(ChannelId::new(channel_id));
                        (ScheduledEventType::Voice, b)
                    } else {
                        let location = cmd.external_location.unwrap_or_default();
                        let end = cmd.end_time.as_deref()
                            .and_then(|s| s.parse::<serenity::model::Timestamp>().ok());
                        let mut b = CreateScheduledEvent::new(ScheduledEventType::External, &cmd.name, start)
                            .location(&location);
                        if let Some(end_ts) = end {
                            b = b.end_time(end_ts);
                        }
                        (ScheduledEventType::External, b)
                    };
                    let _ = kind;
                    if let Some(ref desc) = cmd.description {
                        builder = builder.description(desc);
                    }

                    if let Err(e) = guild.create_scheduled_event(&*http, builder).await {
                        warn!("Failed to create scheduled event '{}' in guild {}: {}", cmd.name, cmd.guild_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize scheduled_event_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_scheduled_event_delete(
        http: Arc<Http>,
        client: async_nats::Client,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::scheduled_event_delete(&prefix);
        let mut stream = subscriber.subscribe::<DeleteScheduledEventCommand>(&subject).await?;
        info!("Listening for scheduled_event_delete commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let event_id = serenity::model::id::ScheduledEventId::new(cmd.event_id);
                    if let Err(e) = guild.delete_scheduled_event(&*http, event_id).await {
                        warn!("Failed to delete scheduled event {} from guild {}: {}", cmd.event_id, cmd.guild_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize scheduled_event_delete command: {}", e),
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
/// - Custom emoji (static): `name:id` (e.g., `wave:123456789`)
/// - Custom emoji (animated): `name:id:1` or `name:id:animated`
fn parse_reaction_type(emoji: &str) -> serenity::model::channel::ReactionType {
    let parts: Vec<&str> = emoji.splitn(3, ':').collect();
    match parts.as_slice() {
        [name, id_str, animated_flag] => {
            if let Ok(id) = id_str.parse::<u64>() {
                let animated = *animated_flag == "1" || *animated_flag == "animated";
                return serenity::model::channel::ReactionType::Custom {
                    animated,
                    id: serenity::model::id::EmojiId::new(id),
                    name: Some(name.to_string()),
                };
            }
        }
        [name, id_str] => {
            if let Ok(id) = id_str.parse::<u64>() {
                return serenity::model::channel::ReactionType::Custom {
                    animated: false,
                    id: serenity::model::id::EmojiId::new(id),
                    name: Some(name.to_string()),
                };
            }
        }
        _ => {}
    }
    serenity::model::channel::ReactionType::Unicode(emoji.to_string())
}
