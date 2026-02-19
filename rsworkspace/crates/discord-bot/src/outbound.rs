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
use discord_types::types::{
    ActionRowComponent, AppInfo, AttachedFile, AuditLogEntryInfo, ButtonStyle as DcButtonStyle,
    ChannelType, DiscordUser, Embed, EmbedAuthor, EmbedField, EmbedFooter, EmbedMedia, Emoji,
    FetchedBan, FetchedChannel, FetchedGuild, FetchedInvite, FetchedMember, FetchedMessage,
    FetchedRole, ScheduledEventUserInfo, VoiceRegionInfo,
};
use discord_types::{
    AddReactionCommand, AddThreadMemberCommand, ArchiveThreadCommand, AssignRoleCommand,
    AutocompleteRespondCommand, BanUserCommand, BulkDeleteMessagesCommand, CreateChannelCommand,
    CreateDmChannelCommand, CreateEmojiCommand, CreateForumPostCommand, CreateInviteCommand,
    CreatePollCommand, CreateRoleCommand, CreateScheduledEventCommand, CreateStageInstanceCommand,
    CreateStickerCommand, CreateThreadCommand, CreateWebhookCommand, CrosspostMessageCommand,
    DeleteChannelCommand, DeleteChannelPermissionsCommand, DeleteEmojiCommand,
    DeleteIntegrationCommand, DeleteInteractionResponseCommand, DeleteMessageCommand,
    DeleteRoleCommand, DeleteScheduledEventCommand, DeleteStageInstanceCommand,
    DeleteStickerCommand, DeleteWebhookCommand, EditChannelCommand, EditEmojiCommand,
    EditInteractionResponseCommand, EditMessageCommand, EditRoleCommand, EditScheduledEventCommand,
    EditStickerCommand, EditWebhookCommand, ExecuteWebhookCommand, FetchAuditLogCommand,
    FetchBansCommand, FetchChannelCommand, FetchEmojisCommand, FetchGuildChannelsCommand,
    FetchGuildCommand, FetchGuildMembersCommand, FetchInvitesCommand, FetchMemberCommand,
    FetchMessagesCommand, FetchPinnedMessagesCommand, FetchRolesCommand,
    FetchScheduledEventUsersCommand, GuildMemberNickCommand, InteractionDeferCommand,
    InteractionFollowupCommand, InteractionRespondCommand, KickUserCommand, ModalRespondCommand,
    PinMessageCommand, PruneMembersCommand, RemoveAllReactionsCommand, RemoveReactionCommand,
    RemoveRoleCommand, RemoveThreadMemberCommand, RevokeInviteCommand, SendMessageCommand,
    SetBotPresenceCommand, SetChannelPermissionsCommand, SyncIntegrationCommand,
    TimeoutUserCommand, TypingCommand, UnbanUserCommand, UnpinMessageCommand,
    VoiceDisconnectCommand, VoiceMoveCommand,
};
use serenity::builder::{
    CreateActionRow, CreateAttachment, CreateAutocompleteResponse, CreateButton, CreateChannel,
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseFollowup, CreateInteractionResponseMessage, CreateMessage, CreatePoll,
    CreatePollAnswer, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
    CreateStageInstance, CreateSticker, CreateThread, EditChannel, EditInteractionResponse,
    EditMember, EditMessage, EditRole, EditScheduledEvent, EditThread, EditWebhook, GetMessages,
};
use serenity::http::Http;
use serenity::model::application::ButtonStyle as SerenityButtonStyle;
use serenity::model::channel::{PermissionOverwrite, PermissionOverwriteType};
use serenity::model::id::{ChannelId, GuildId, MessageId, RoleId, TargetId, UserId, WebhookId};
use serenity::model::permissions::Permissions;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::bridge::PairingState;
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

/// Split text into chunks no longer than `max_len` characters and optionally no more
/// than `max_lines` newlines per chunk.
/// Breaks preferably at newlines, then spaces; falls back to a hard character boundary.
fn chunk_text(text: &str, max_len: usize, max_lines: Option<usize>) -> Vec<String> {
    // First split by line limit if requested
    let line_chunks: Vec<String> = if let Some(limit) = max_lines {
        let lines: Vec<&str> = text.split('\n').collect();
        let mut result = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let end = (i + limit).min(lines.len());
            result.push(lines[i..end].join("\n"));
            i = end;
        }
        result
    } else {
        vec![text.to_string()]
    };

    // Then split each line-chunk by character limit
    let mut chunks = Vec::new();
    for segment in &line_chunks {
        if segment.len() <= max_len {
            if !segment.is_empty() {
                chunks.push(segment.clone());
            }
            continue;
        }
        let mut remaining: &str = segment;
        while !remaining.is_empty() {
            if remaining.len() <= max_len {
                chunks.push(remaining.to_string());
                break;
            }
            // Find the last valid char boundary within max_len
            let mut boundary = max_len;
            while !remaining.is_char_boundary(boundary) {
                boundary -= 1;
            }
            let slice = &remaining[..boundary];
            let break_pos = slice
                .rfind('\n')
                .or_else(|| slice.rfind(' '))
                .unwrap_or(boundary);
            let mut bp = break_pos;
            while bp > 0 && !remaining.is_char_boundary(bp) {
                bp -= 1;
            }
            if bp == 0 {
                bp = boundary; // hard break if no whitespace found
            }
            chunks.push(remaining[..bp].to_string());
            remaining = remaining[bp..].trim_start_matches(|c: char| c == '\n' || c == ' ');
        }
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// Extract a `[[reply_to:<id>]]` tag from message content.
/// Returns the cleaned content (with the tag removed) and the optional message ID.
/// `[[reply_to_current]]` is stripped but ignored (the agent concern).
fn extract_reply_tag(content: &str) -> (String, Option<u64>) {
    // Match [[reply_to:<numeric_id>]] and strip it from content
    if let Some(start) = content.find("[[reply_to:") {
        let rest = &content[start + "[[reply_to:".len()..];
        if let Some(end) = rest.find("]]") {
            let id_str = &rest[..end];
            let reply_id = id_str.parse::<u64>().ok();
            let tag = format!("[[reply_to:{}]]", id_str);
            let cleaned = content.replacen(&tag, "", 1).trim_start().to_string();
            return (cleaned, reply_id);
        }
    }
    // Strip [[reply_to_current]] if present (not actionable at bot level)
    if content.contains("[[reply_to_current]]") {
        let cleaned = content
            .replacen("[[reply_to_current]]", "", 1)
            .trim_start()
            .to_string();
        return (cleaned, None);
    }
    (content.to_string(), None)
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
            let has_select = row.components.iter().any(|c| {
                matches!(
                    c,
                    ActionRowComponent::StringSelect(_)
                        | ActionRowComponent::UserSelect(_)
                        | ActionRowComponent::RoleSelect(_)
                        | ActionRowComponent::ChannelSelect(_)
                )
            });

            if has_buttons && !has_select {
                let buttons: Vec<CreateButton> = row
                    .components
                    .iter()
                    .filter_map(|c| {
                        if let ActionRowComponent::Button(btn) = c {
                            let mut b = match btn.style {
                                DcButtonStyle::Link => {
                                    CreateButton::new_link(btn.url.as_deref().unwrap_or(""))
                                }
                                _ => {
                                    let style = match btn.style {
                                        DcButtonStyle::Secondary => SerenityButtonStyle::Secondary,
                                        DcButtonStyle::Success => SerenityButtonStyle::Success,
                                        DcButtonStyle::Danger => SerenityButtonStyle::Danger,
                                        _ => SerenityButtonStyle::Primary,
                                    };
                                    CreateButton::new(btn.custom_id.as_deref().unwrap_or(""))
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
                                let mut o = CreateSelectMenuOption::new(&opt.label, &opt.value);
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
                    ActionRowComponent::UserSelect(m) => Some((
                        m,
                        CreateSelectMenuKind::User {
                            default_users: None,
                        },
                    )),
                    ActionRowComponent::RoleSelect(m) => Some((
                        m,
                        CreateSelectMenuKind::Role {
                            default_roles: None,
                        },
                    )),
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
                thumbnail: e
                    .thumbnail
                    .as_ref()
                    .map(|t| EmbedMedia { url: t.url.clone() }),
                timestamp: e.timestamp.map(|t| t.to_string()),
            })
            .collect(),
    }
}

/// Processes NATS agent commands and forwards them to Discord
pub struct OutboundProcessor<N>
where
    N: trogon_nats::SubscribeClient + trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    pub(crate) http: Arc<Http>,
    pub(crate) client: N,
    pub(crate) publisher: MessagePublisher,
    pub(crate) prefix: String,
    pub(crate) streaming_messages: StreamingMessages,
    pub(crate) shard_manager: Option<Arc<serenity::all::ShardManager>>,
    pub(crate) pairing_state: Arc<PairingState>,
    pub(crate) response_prefix: Option<String>,
    pub(crate) reply_to_mode: crate::config::ReplyToMode,
    pub(crate) max_lines_per_message: Option<usize>,
}

impl<N> OutboundProcessor<N>
where
    N: trogon_nats::SubscribeClient + trogon_nats::PublishClient + Clone + Send + Sync + 'static,
{
    pub fn new(
        http: Arc<Http>,
        client: N,
        publisher: MessagePublisher,
        prefix: String,
        shard_manager: Option<Arc<serenity::all::ShardManager>>,
        pairing_state: Arc<PairingState>,
        response_prefix: Option<String>,
        reply_to_mode: crate::config::ReplyToMode,
        max_lines_per_message: Option<usize>,
    ) -> Self {
        Self {
            http,
            client,
            publisher,
            prefix,
            streaming_messages: Arc::new(RwLock::new(HashMap::new())),
            shard_manager,
            pairing_state,
            response_prefix,
            reply_to_mode,
            max_lines_per_message,
        }
    }

    /// Start all command handler tasks
    pub async fn run(self) -> Result<()> {
        let http = self.http;
        let client = self.client;
        let prefix = self.prefix;
        let streaming_messages = self.streaming_messages;
        let shard_manager = self.shard_manager;
        let pairing_state = self.pairing_state;
        let response_prefix = self.response_prefix;
        let reply_to_mode = self.reply_to_mode;
        let max_lines_per_message = self.max_lines_per_message;
        let publisher = self.publisher;

        info!("Starting outbound processor for prefix: {}", prefix);

        let (
            r1,
            r2,
            r3,
            r4,
            r5,
            r6,
            r7,
            r8,
            r9,
            r10,
            r11,
            r12,
            r13,
            r14,
            r15,
            r16,
            r17,
            r18,
            r19,
            r20,
            r21,
            r22,
            r23,
            r24,
            r25,
            r26,
            r27,
            r28,
            r29,
            r30,
            r31,
            r32,
            r33,
            r34,
            r35,
            r36,
            r37,
            r38,
            r39,
            r40,
            r41,
            r42,
            r43,
            r44,
            r45,
            r46,
            r47,
            r48,
            r49,
            r50,
            r51,
            r52,
            r53,
            r54,
            r55,
            r56,
            r57,
            r58,
            r59,
            r60,
            r61,
            r62,
            r63,
            r64,
            r65,
            r66,
            r67,
            r68,
            r69,
            r70,
            r71,
            r72,
            r73,
            r74,
            r75,
            r76,
            r77,
            r78,
            r79,
            r80,
            r81,
        ) = tokio::join!(
            Self::handle_send_messages(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone(),
                response_prefix,
                reply_to_mode,
                max_lines_per_message,
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
            Self::handle_ban(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_kick(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_timeout(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_create_channel(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_channel(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_delete_channel(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_create_role(http.clone(), client.clone(), prefix.clone()),
            Self::handle_assign_role(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_remove_role(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_delete_role(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_pin(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_unpin(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_bulk_delete(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_create_thread(http.clone(), client.clone(), prefix.clone()),
            Self::handle_archive_thread(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_bot_presence(client.clone(), prefix.clone(), shard_manager),
            Self::handle_fetch_messages(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_member(http.clone(), client.clone(), prefix.clone()),
            Self::handle_unban(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_guild_member_nick(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_webhook_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_webhook_execute(http.clone(), client.clone(), prefix.clone()),
            Self::handle_webhook_delete(http.clone(), client.clone(), prefix.clone()),
            Self::handle_voice_move(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_voice_disconnect(
                http.clone(),
                client.clone(),
                prefix.clone(),
                publisher.clone()
            ),
            Self::handle_invite_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_invite_revoke(http.clone(), client.clone(), prefix.clone()),
            Self::handle_emoji_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_emoji_delete(http.clone(), client.clone(), prefix.clone()),
            Self::handle_scheduled_event_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_scheduled_event_delete(http.clone(), client.clone(), prefix.clone()),
            Self::handle_reaction_remove_all(http.clone(), client.clone(), prefix.clone()),
            Self::handle_message_crosspost(http.clone(), client.clone(), prefix.clone()),
            Self::handle_forum_post_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_thread_member_add(http.clone(), client.clone(), prefix.clone()),
            Self::handle_thread_member_remove(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_role(http.clone(), client.clone(), prefix.clone()),
            Self::handle_stage_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_stage_delete(http.clone(), client.clone(), prefix.clone()),
            Self::handle_webhook_edit(http.clone(), client.clone(), prefix.clone()),
            Self::handle_dm_create(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_guild(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_channel(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_invites(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_scheduled_event(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_emoji(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_guild_members(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_guild_channels(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_pinned(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_roles(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_interaction_response(http.clone(), client.clone(), prefix.clone()),
            Self::handle_delete_interaction_response(http.clone(), client.clone(), prefix.clone()),
            Self::handle_set_channel_permissions(http.clone(), client.clone(), prefix.clone()),
            Self::handle_delete_channel_permissions(http.clone(), client.clone(), prefix.clone()),
            Self::handle_prune_members(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_audit_log(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_scheduled_event_users(http.clone(), client.clone(), prefix.clone()),
            Self::handle_edit_sticker(http.clone(), client.clone(), prefix.clone()),
            Self::handle_delete_sticker(http.clone(), client.clone(), prefix.clone()),
            Self::handle_delete_integration(http.clone(), client.clone(), prefix.clone()),
            Self::handle_sync_integration(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_voice_regions(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_application_info(http.clone(), client.clone(), prefix.clone()),
            Self::handle_pairing_approve(
                http.clone(),
                client.clone(),
                prefix.clone(),
                pairing_state.clone(),
                publisher.clone()
            ),
            Self::handle_pairing_reject(
                http.clone(),
                client.clone(),
                prefix.clone(),
                pairing_state.clone(),
                publisher.clone()
            ),
            Self::handle_create_poll(http.clone(), client.clone(), prefix.clone()),
            Self::handle_create_sticker(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_emojis(http.clone(), client.clone(), prefix.clone()),
            Self::handle_fetch_bans(http.clone(), client.clone(), prefix.clone()),
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
            ("reaction_remove_all", r44),
            ("message_crosspost", r45),
            ("forum_post_create", r46),
            ("thread_member_add", r47),
            ("thread_member_remove", r48),
            ("edit_role", r49),
            ("stage_create", r50),
            ("stage_delete", r51),
            ("webhook_edit", r52),
            ("dm_create", r53),
            ("fetch_guild", r54),
            ("fetch_channel", r55),
            ("fetch_invites", r56),
            ("edit_scheduled_event", r57),
            ("edit_emoji", r58),
            ("fetch_guild_members", r59),
            ("fetch_guild_channels", r60),
            ("fetch_pinned", r61),
            ("fetch_roles", r62),
            ("edit_interaction_response", r63),
            ("delete_interaction_response", r64),
            ("set_channel_permissions", r65),
            ("delete_channel_permissions", r66),
            ("prune_members", r67),
            ("fetch_audit_log", r68),
            ("fetch_scheduled_event_users", r69),
            ("edit_sticker", r70),
            ("delete_sticker", r71),
            ("delete_integration", r72),
            ("sync_integration", r73),
            ("fetch_voice_regions", r74),
            ("fetch_application_info", r75),
            ("pairing_approve", r76),
            ("pairing_reject", r77),
            ("create_poll", r78),
            ("create_sticker", r79),
            ("fetch_emojis", r80),
            ("fetch_bans", r81),
        ] {
            if let Err(e) = result {
                error!("Outbound handler '{}' exited with error: {}", name, e);
            }
        }

        Ok(())
    }

    async fn handle_send_messages(
        http: Arc<Http>,
        client: N,
        prefix: String,
        publisher: MessagePublisher,
        response_prefix: Option<String>,
        reply_to_mode: crate::config::ReplyToMode,
        max_lines_per_message: Option<usize>,
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

                    if cmd.as_voice && !attachments.is_empty() {
                        // Voice messages: empty content + IS_VOICE_MESSAGE flag (bit 13)
                        let builder = CreateMessage::new()
                            .content("")
                            .flags(serenity::model::channel::MessageFlags::from_bits_truncate(
                                1 << 13,
                            ))
                            .add_files(attachments);
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
                    } else {
                        // Parse [[reply_to:<id>]] tag from content if present.
                        // Strip the tag and use it as reply_to_message_id if not already set.
                        let (content_clean, tag_reply_id) = extract_reply_tag(&cmd.content);

                        // Text message: prepend response_prefix then chunk if needed
                        let full_content = match &response_prefix {
                            Some(rp) => format!("{}{}", rp, content_clean),
                            None => content_clean,
                        };

                        // Determine effective reply_to_message_id: cmd field takes priority,
                        // then the [[reply_to:<id>]] tag.
                        let effective_reply_id = cmd.reply_to_message_id.or(tag_reply_id);

                        let chunks =
                            chunk_text(&full_content, MAX_DISCORD_LEN, max_lines_per_message);
                        let n = chunks.len();

                        for (i, chunk) in chunks.iter().enumerate() {
                            let is_first = i == 0;
                            let is_last = i == n - 1;

                            let mut b = CreateMessage::new().content(chunk.as_str());

                            // Apply reply reference according to reply_to_mode
                            let should_reply = match reply_to_mode {
                                crate::config::ReplyToMode::Off => false,
                                crate::config::ReplyToMode::First => is_first,
                                crate::config::ReplyToMode::All => true,
                            };
                            if should_reply {
                                if let Some(reply_id) = effective_reply_id {
                                    b = b.reference_message((channel, MessageId::new(reply_id)));
                                }
                            }

                            if is_last {
                                if !cmd.embeds.is_empty() {
                                    let embeds: Vec<CreateEmbed> =
                                        cmd.embeds.iter().map(build_embed).collect();
                                    b = b.embeds(embeds);
                                }
                                if !cmd.components.is_empty() {
                                    b = b.components(build_action_rows(&cmd.components));
                                }
                                if !attachments.is_empty() {
                                    b = b.add_files(attachments.clone());
                                }
                            }

                            for attempt in 0..=MAX_RETRIES {
                                match channel.send_message(&*http, b.clone()).await {
                                    Ok(_) => break,
                                    Err(e) => match classify(&subject, &e) {
                                        ErrorOutcome::Retry(dur) if attempt < MAX_RETRIES => {
                                            tokio::time::sleep(dur).await;
                                        }
                                        outcome => {
                                            publish_if_permanent(
                                                &publisher,
                                                &error_subject,
                                                &outcome,
                                            )
                                            .await;
                                            log_outcome(&subject, &context, outcome);
                                            break;
                                        }
                                    },
                                }
                            }
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
        client: N,
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
                            let embeds: Vec<CreateEmbed> =
                                cmd.embeds.iter().map(build_embed).collect();
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
        client: N,
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

    async fn handle_interaction_respond(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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

    async fn handle_interaction_defer(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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
        client: N,
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
        client: N,
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
        client: N,
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

    async fn handle_typing(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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

    async fn handle_modal_respond(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::all::InputTextStyle;
        use serenity::builder::{CreateActionRow, CreateInputText};

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
                    let modal = CreateModal::new(&cmd.custom_id, &cmd.title).components(components);
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

    async fn handle_autocomplete_respond(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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
        client: N,
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
                    let context = format!(
                        "Failed to ban user {} in guild {}",
                        cmd.user_id, cmd.guild_id
                    );
                    // dmd = delete message days (max 7), API expects u8
                    let delete_days = (cmd.delete_message_seconds / 86400).min(7) as u8;
                    let reason = cmd.reason.as_deref().unwrap_or("No reason provided");
                    for attempt in 0..=MAX_RETRIES {
                        match guild
                            .ban_with_reason(http.as_ref(), user, delete_days, reason)
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
                Err(e) => warn!("Failed to deserialize ban command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_kick(
        http: Arc<Http>,
        client: N,
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
                    let context = format!(
                        "Failed to kick user {} from guild {}",
                        cmd.user_id, cmd.guild_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match guild.kick_with_reason(http.as_ref(), user, reason).await {
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
                Err(e) => warn!("Failed to deserialize kick command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_timeout(
        http: Arc<Http>,
        client: N,
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
                    let context = format!(
                        "Failed to timeout user {} in guild {}",
                        cmd.user_id, cmd.guild_id
                    );

                    let edit = if cmd.duration_secs == 0 {
                        EditMember::new().enable_communication()
                    } else {
                        let now_secs = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let until_secs = (now_secs + cmd.duration_secs) as i64;
                        let timestamp = serenity::model::Timestamp::from_unix_timestamp(until_secs)
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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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

    async fn handle_create_channel(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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
            let context = format!(
                "Failed to create channel '{}' in guild {}",
                cmd.name, cmd.guild_id
            );
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
                                .publish_with_headers(
                                    reply,
                                    async_nats::HeaderMap::new(),
                                    created_channel.id.get().to_string().into(),
                                )
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
        client: N,
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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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
        client: N,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::channel_delete(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteChannelCommand>(&subject)
            .await?;

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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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

    async fn handle_create_role(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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
            let context = format!(
                "Failed to create role '{}' in guild {}",
                cmd.name, cmd.guild_id
            );
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
                                .publish_with_headers(
                                    reply,
                                    async_nats::HeaderMap::new(),
                                    created_role.id.get().to_string().into(),
                                )
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
        client: N,
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
                    let context = format!(
                        "Failed to assign role {} to user {} in guild {}",
                        cmd.role_id, cmd.user_id, cmd.guild_id
                    );
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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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
        client: N,
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
                    let context = format!(
                        "Failed to remove role {} from user {} in guild {}",
                        cmd.role_id, cmd.user_id, cmd.guild_id
                    );
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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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
        client: N,
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
                    let context = format!(
                        "Failed to delete role {} in guild {}",
                        cmd.role_id, cmd.guild_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match guild.delete_role(http.as_ref(), role).await {
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
                Err(e) => warn!("Failed to deserialize delete_role command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_pin(
        http: Arc<Http>,
        client: N,
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
                    let context = format!(
                        "Failed to pin message {} in channel {}",
                        cmd.message_id, cmd.channel_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match channel.pin(http.as_ref(), message).await {
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
                Err(e) => warn!("Failed to deserialize pin command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_unpin(
        http: Arc<Http>,
        client: N,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_unpin(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber
            .subscribe::<UnpinMessageCommand>(&subject)
            .await?;

        info!("Listening for unpin commands on {}", subject);

        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let message = MessageId::new(cmd.message_id);
                    let context = format!(
                        "Failed to unpin message {} in channel {}",
                        cmd.message_id, cmd.channel_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match channel.unpin(http.as_ref(), message).await {
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
                Err(e) => warn!("Failed to deserialize unpin command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_bulk_delete(
        http: Arc<Http>,
        client: N,
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
                    let context = format!(
                        "Failed to bulk delete {} messages in channel {}",
                        message_ids.len(),
                        cmd.channel_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match channel.delete_messages(http.as_ref(), &message_ids).await {
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
                Err(e) => warn!("Failed to deserialize bulk_delete command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_create_thread(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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
            let context = format!(
                "Failed to create thread '{}' in channel {}",
                cmd.name, cmd.channel_id
            );
            let archive_dur = match cmd.auto_archive_mins {
                60 => AutoArchiveDuration::OneHour,
                4320 => AutoArchiveDuration::ThreeDays,
                10080 => AutoArchiveDuration::OneWeek,
                _ => AutoArchiveDuration::OneDay,
            };
            let builder = CreateThread::new(&cmd.name).auto_archive_duration(archive_dur);
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
                                .publish_with_headers(
                                    reply,
                                    async_nats::HeaderMap::new(),
                                    created_thread.id.get().to_string().into(),
                                )
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
        client: N,
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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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
        client: N,
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
                        ActivityData::streaming(a.name.clone(), url)
                            .unwrap_or_else(|_| ActivityData::playing(a.name))
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
    async fn handle_fetch_messages(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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

            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
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
    async fn handle_fetch_member(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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

            let response_bytes = match guild.member(http.as_ref(), user).await {
                Ok(member) => {
                    let fetched = FetchedMember {
                        user: DiscordUser {
                            id: member.user.id.get(),
                            username: member.user.name.clone(),
                            global_name: member.user.global_name.as_deref().map(String::from),
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

            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_member: failed to send reply: {}", e);
            }
        }

        Ok(())
    }

    async fn handle_unban(
        http: Arc<Http>,
        client: N,
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
                    let context = format!(
                        "Failed to unban user {} in guild {}",
                        cmd.user_id, cmd.guild_id
                    );
                    for attempt in 0..=MAX_RETRIES {
                        match http.remove_ban(guild, user, None).await {
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
                Err(e) => warn!("Failed to deserialize unban command: {}", e),
            }
        }

        Ok(())
    }

    async fn handle_guild_member_nick(
        http: Arc<Http>,
        client: N,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::guild_member_nick(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber
            .subscribe::<GuildMemberNickCommand>(&subject)
            .await?;

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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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

    async fn handle_webhook_create(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::webhook_create(&prefix);
        let mut stream = subscriber
            .subscribe::<CreateWebhookCommand>(&subject)
            .await?;
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

    async fn handle_webhook_execute(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::webhook_execute(&prefix);
        let mut stream = subscriber
            .subscribe::<ExecuteWebhookCommand>(&subject)
            .await?;
        info!("Listening for webhook_execute commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let mut builder = serenity::builder::ExecuteWebhook::new().content(cmd.content);
                    if let Some(ref name) = cmd.username {
                        builder = builder.username(name);
                    }
                    if let Some(ref url) = cmd.avatar_url {
                        builder = builder.avatar_url(url);
                    }
                    if let Err(e) = http
                        .execute_webhook(
                            cmd.webhook_id.into(),
                            None,
                            &cmd.webhook_token,
                            false,
                            Vec::new(),
                            &builder,
                        )
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

    async fn handle_webhook_delete(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::webhook_delete(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteWebhookCommand>(&subject)
            .await?;
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
        client: N,
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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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
        client: N,
        prefix: String,
        publisher: MessagePublisher,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::voice_disconnect(&prefix);
        let error_subject = subjects::bot::command_error(&prefix);
        let mut stream = subscriber
            .subscribe::<VoiceDisconnectCommand>(&subject)
            .await?;
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
                                    publish_if_permanent(&publisher, &error_subject, &outcome)
                                        .await;
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

    async fn handle_invite_create(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::invite_create(&prefix);
        let mut stream = subscriber
            .subscribe::<CreateInviteCommand>(&subject)
            .await?;
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
                        warn!(
                            "Failed to create invite for channel {}: {}",
                            cmd.channel_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize invite_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_invite_revoke(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::invite_revoke(&prefix);
        let mut stream = subscriber
            .subscribe::<RevokeInviteCommand>(&subject)
            .await?;
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

    async fn handle_emoji_create(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::emoji_create(&prefix);
        let mut stream = subscriber.subscribe::<CreateEmojiCommand>(&subject).await?;
        info!("Listening for emoji_create commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    if let Err(e) = guild.create_emoji(&*http, &cmd.name, &cmd.image_data).await {
                        warn!(
                            "Failed to create emoji '{}' in guild {}: {}",
                            cmd.name, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize emoji_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_emoji_delete(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
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
                        warn!(
                            "Failed to delete emoji {} from guild {}: {}",
                            cmd.emoji_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize emoji_delete command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_scheduled_event_create(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        use serenity::builder::CreateScheduledEvent;
        use serenity::model::guild::ScheduledEventType;

        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::scheduled_event_create(&prefix);
        let mut stream = subscriber
            .subscribe::<CreateScheduledEventCommand>(&subject)
            .await?;
        info!(
            "Listening for scheduled_event_create commands on {}",
            subject
        );
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
                        let b =
                            CreateScheduledEvent::new(ScheduledEventType::Voice, &cmd.name, start)
                                .channel_id(ChannelId::new(channel_id));
                        (ScheduledEventType::Voice, b)
                    } else {
                        let location = cmd.external_location.unwrap_or_default();
                        let end = cmd
                            .end_time
                            .as_deref()
                            .and_then(|s| s.parse::<serenity::model::Timestamp>().ok());
                        let mut b = CreateScheduledEvent::new(
                            ScheduledEventType::External,
                            &cmd.name,
                            start,
                        )
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
                        warn!(
                            "Failed to create scheduled event '{}' in guild {}: {}",
                            cmd.name, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to deserialize scheduled_event_create command: {}",
                    e
                ),
            }
        }
        Ok(())
    }

    async fn handle_scheduled_event_delete(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::scheduled_event_delete(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteScheduledEventCommand>(&subject)
            .await?;
        info!(
            "Listening for scheduled_event_delete commands on {}",
            subject
        );
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let event_id = serenity::model::id::ScheduledEventId::new(cmd.event_id);
                    if let Err(e) = guild.delete_scheduled_event(&*http, event_id).await {
                        warn!(
                            "Failed to delete scheduled event {} from guild {}: {}",
                            cmd.event_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to deserialize scheduled_event_delete command: {}",
                    e
                ),
            }
        }
        Ok(())
    }

    async fn handle_reaction_remove_all(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::reaction_remove_all(&prefix);
        let mut stream = subscriber
            .subscribe::<RemoveAllReactionsCommand>(&subject)
            .await?;
        info!("Listening for reaction_remove_all commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let msg = MessageId::new(cmd.message_id);
                    if let Some(ref emoji_str) = cmd.emoji {
                        // Parse as ReactionType and remove only that emoji
                        match emoji_str.parse::<serenity::model::channel::ReactionType>() {
                            Ok(rt) => {
                                if let Err(e) =
                                    http.delete_message_reaction_emoji(channel, msg, &rt).await
                                {
                                    warn!(
                                        "Failed to remove emoji reactions {} from {}/{}: {}",
                                        emoji_str, cmd.channel_id, cmd.message_id, e
                                    );
                                }
                            }
                            Err(e) => warn!("Failed to parse emoji '{}': {}", emoji_str, e),
                        }
                    } else {
                        if let Err(e) = http.delete_message_reactions(channel, msg).await {
                            warn!(
                                "Failed to remove all reactions from {}/{}: {}",
                                cmd.channel_id, cmd.message_id, e
                            );
                        }
                    }
                }
                Err(e) => warn!("Failed to deserialize reaction_remove_all command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_message_crosspost(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::message_crosspost(&prefix);
        let mut stream = subscriber
            .subscribe::<CrosspostMessageCommand>(&subject)
            .await?;
        info!("Listening for message_crosspost commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let msg = MessageId::new(cmd.message_id);
                    if let Err(e) = http.crosspost_message(channel, msg).await {
                        warn!(
                            "Failed to crosspost message {}/{}: {}",
                            cmd.channel_id, cmd.message_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize message_crosspost command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_forum_post_create(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::builder::{CreateForumPost, CreateMessage};

        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::forum_post_create(&prefix);
        let mut stream = subscriber
            .subscribe::<CreateForumPostCommand>(&subject)
            .await?;
        info!("Listening for forum_post_create commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let msg = CreateMessage::new().content(truncate(&cmd.content));
                    let mut builder = CreateForumPost::new(&cmd.name, msg);
                    if !cmd.tags.is_empty() {
                        let tag_ids: Vec<serenity::model::id::ForumTagId> = cmd
                            .tags
                            .iter()
                            .map(|&id| serenity::model::id::ForumTagId::new(id))
                            .collect();
                        builder = builder.set_applied_tags(tag_ids);
                    }
                    if let Err(e) = channel.create_forum_post(&*http, builder).await {
                        warn!(
                            "Failed to create forum post '{}' in {}: {}",
                            cmd.name, cmd.channel_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize forum_post_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_thread_member_add(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::thread_member_add(&prefix);
        let mut stream = subscriber
            .subscribe::<AddThreadMemberCommand>(&subject)
            .await?;
        info!("Listening for thread_member_add commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let thread = ChannelId::new(cmd.thread_id);
                    let user = UserId::new(cmd.user_id);
                    if let Err(e) = http.add_thread_channel_member(thread, user).await {
                        warn!(
                            "Failed to add user {} to thread {}: {}",
                            cmd.user_id, cmd.thread_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize thread_member_add command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_thread_member_remove(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::thread_member_remove(&prefix);
        let mut stream = subscriber
            .subscribe::<RemoveThreadMemberCommand>(&subject)
            .await?;
        info!("Listening for thread_member_remove commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let thread = ChannelId::new(cmd.thread_id);
                    let user = UserId::new(cmd.user_id);
                    if let Err(e) = http.remove_thread_channel_member(thread, user).await {
                        warn!(
                            "Failed to remove user {} from thread {}: {}",
                            cmd.user_id, cmd.thread_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize thread_member_remove command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_edit_role(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::role_edit(&prefix);
        let mut stream = subscriber.subscribe::<EditRoleCommand>(&subject).await?;
        info!("Listening for edit_role commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let role = RoleId::new(cmd.role_id);
                    let mut builder = EditRole::new();
                    if let Some(ref name) = cmd.name {
                        builder = builder.name(name);
                    }
                    if let Some(color) = cmd.color {
                        builder = builder.colour(color);
                    }
                    if let Some(hoist) = cmd.hoist {
                        builder = builder.hoist(hoist);
                    }
                    if let Some(mentionable) = cmd.mentionable {
                        builder = builder.mentionable(mentionable);
                    }
                    if let Err(e) = http.edit_role(guild, role, &builder, None).await {
                        warn!(
                            "Failed to edit role {} in guild {}: {}",
                            cmd.role_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize edit_role command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_stage_create(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::builder::Builder;
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::stage_create(&prefix);
        let mut stream = subscriber
            .subscribe::<CreateStageInstanceCommand>(&subject)
            .await?;
        info!("Listening for stage_create commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let builder = CreateStageInstance::new(&cmd.topic);
                    if let Err(e) = builder.execute(&*http, channel).await {
                        warn!(
                            "Failed to create stage instance in {}: {}",
                            cmd.channel_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize stage_create command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_stage_delete(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::stage_delete(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteStageInstanceCommand>(&subject)
            .await?;
        info!("Listening for stage_delete commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    if let Err(e) = http.delete_stage_instance(channel, None).await {
                        warn!(
                            "Failed to delete stage instance in {}: {}",
                            cmd.channel_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize stage_delete command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_webhook_edit(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::webhook_edit(&prefix);
        let mut stream = subscriber.subscribe::<EditWebhookCommand>(&subject).await?;
        info!("Listening for webhook_edit commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let webhook_id = WebhookId::new(cmd.webhook_id);
                    let mut builder = EditWebhook::new();
                    if let Some(ref name) = cmd.name {
                        builder = builder.name(name);
                    }
                    if let Some(channel_id) = cmd.channel_id {
                        builder = builder.channel_id(ChannelId::new(channel_id));
                    }
                    if let Err(e) = http
                        .edit_webhook_with_token(webhook_id, &cmd.webhook_token, &builder, None)
                        .await
                    {
                        warn!("Failed to edit webhook {}: {}", cmd.webhook_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize webhook_edit command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_dm_create(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::dm_create(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for dm_create requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("dm_create: received message without reply subject");
                    continue;
                }
            };
            let cmd: CreateDmChannelCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("dm_create: failed to deserialize: {}", e);
                    continue;
                }
            };
            let user = UserId::new(cmd.user_id);
            let response_bytes = match user.create_dm_channel(&*http).await {
                Ok(channel) => serde_json::to_vec(&Some(channel.id.get())).unwrap_or_default(),
                Err(e) => {
                    error!(
                        "dm_create: Discord API error for user {}: {}",
                        cmd.user_id, e
                    );
                    serde_json::to_vec::<Option<u64>>(&None).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("dm_create: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_guild(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_guild(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_guild requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_guild: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchGuildCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_guild: failed to deserialize: {}", e);
                    continue;
                }
            };
            let response_bytes = match http.get_guild(GuildId::new(cmd.guild_id)).await {
                Ok(g) => {
                    let fetched = FetchedGuild {
                        id: g.id.get(),
                        name: g.name.clone(),
                        member_count: g.approximate_member_count.unwrap_or(0),
                    };
                    serde_json::to_vec(&Some(fetched)).unwrap_or_default()
                }
                Err(e) => {
                    error!("fetch_guild: Discord API error for {}: {}", cmd.guild_id, e);
                    serde_json::to_vec::<Option<FetchedGuild>>(&None).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_guild: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_channel(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        use serenity::model::channel::Channel;
        use serenity::model::channel::ChannelType as SerenityChannelType;
        let subject = subjects::agent::fetch_channel(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_channel requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_channel: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchChannelCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_channel: failed to deserialize: {}", e);
                    continue;
                }
            };
            let response_bytes = match http.get_channel(ChannelId::new(cmd.channel_id)).await {
                Ok(ch) => {
                    let fetched = match ch {
                        Channel::Guild(g) => FetchedChannel {
                            id: g.id.get(),
                            channel_type: match g.kind {
                                SerenityChannelType::Text => ChannelType::GuildText,
                                SerenityChannelType::Voice => ChannelType::GuildVoice,
                                SerenityChannelType::Category => ChannelType::GuildCategory,
                                SerenityChannelType::News => ChannelType::GuildNews,
                                SerenityChannelType::Stage => ChannelType::GuildStageVoice,
                                SerenityChannelType::Forum => ChannelType::GuildForum,
                                _ => ChannelType::Unknown,
                            },
                            guild_id: Some(g.guild_id.get()),
                            name: Some(g.name.clone()),
                            topic: g.topic.clone(),
                        },
                        Channel::Private(p) => FetchedChannel {
                            id: p.id.get(),
                            channel_type: ChannelType::Dm,
                            guild_id: None,
                            name: Some(p.name()),
                            topic: None,
                        },
                        _ => FetchedChannel {
                            id: ch.id().get(),
                            channel_type: ChannelType::Unknown,
                            guild_id: None,
                            name: None,
                            topic: None,
                        },
                    };
                    serde_json::to_vec(&Some(fetched)).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_channel: Discord API error for {}: {}",
                        cmd.channel_id, e
                    );
                    serde_json::to_vec::<Option<FetchedChannel>>(&None).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_channel: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_invites(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_invites(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_invites requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_invites: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchInvitesCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_invites: failed to deserialize: {}", e);
                    continue;
                }
            };
            let channel = ChannelId::new(cmd.channel_id);
            let response_bytes = match channel.invites(&*http).await {
                Ok(invites) => {
                    let fetched: Vec<FetchedInvite> = invites
                        .iter()
                        .map(|inv| FetchedInvite {
                            code: inv.code.clone(),
                            channel_id: inv.channel.id.get(),
                            inviter_id: inv.inviter.as_ref().map(|u| u.id.get()),
                            uses: inv.uses,
                            max_uses: inv.max_uses as u64,
                            max_age_secs: inv.max_age as u64,
                            temporary: inv.temporary,
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_invites: Discord API error for {}: {}",
                        cmd.channel_id, e
                    );
                    serde_json::to_vec::<Vec<FetchedInvite>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_invites: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_edit_scheduled_event(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::scheduled_event_edit(&prefix);
        let mut stream = subscriber
            .subscribe::<EditScheduledEventCommand>(&subject)
            .await?;
        info!("Listening for scheduled_event_edit commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let event_id = serenity::model::id::ScheduledEventId::new(cmd.event_id);
                    let mut builder = EditScheduledEvent::new();
                    if let Some(ref name) = cmd.name {
                        builder = builder.name(name);
                    }
                    if let Some(ref desc) = cmd.description {
                        builder = builder.description(desc);
                    }
                    if let Some(ref start) = cmd.start_time {
                        if let Ok(ts) = start.parse::<serenity::model::Timestamp>() {
                            builder = builder.start_time(ts);
                        }
                    }
                    if let Err(e) = http
                        .edit_scheduled_event(guild, event_id, &builder, None)
                        .await
                    {
                        warn!(
                            "Failed to edit scheduled event {} in guild {}: {}",
                            cmd.event_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize scheduled_event_edit command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_edit_emoji(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::model::id::EmojiId;
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::emoji_edit(&prefix);
        let mut stream = subscriber.subscribe::<EditEmojiCommand>(&subject).await?;
        info!("Listening for emoji_edit commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let emoji_id = EmojiId::new(cmd.emoji_id);
                    let map = serde_json::json!({ "name": cmd.name });
                    if let Err(e) = http.edit_emoji(guild, emoji_id, &map, None).await {
                        warn!(
                            "Failed to edit emoji {} in guild {}: {}",
                            cmd.emoji_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize emoji_edit command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_fetch_guild_members(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_guild_members(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_guild_members requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_guild_members: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchGuildMembersCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_guild_members: failed to deserialize: {}", e);
                    continue;
                }
            };
            let guild = GuildId::new(cmd.guild_id);
            let response_bytes = match http.get_guild_members(guild, cmd.limit, cmd.after_id).await
            {
                Ok(members) => {
                    let fetched: Vec<FetchedMember> = members
                        .iter()
                        .map(|m| FetchedMember {
                            user: DiscordUser {
                                id: m.user.id.get(),
                                username: m.user.name.clone(),
                                global_name: m.user.global_name.as_deref().map(String::from),
                                bot: m.user.bot,
                            },
                            guild_id: cmd.guild_id,
                            nick: m.nick.clone(),
                            roles: m.roles.iter().map(|r| r.get()).collect(),
                            joined_at: m.joined_at.and_then(|t| t.to_rfc3339()),
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_guild_members: Discord API error for guild {}: {}",
                        cmd.guild_id, e
                    );
                    serde_json::to_vec::<Vec<FetchedMember>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_guild_members: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_guild_channels(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        use serenity::model::channel::ChannelType as SerenityChannelType;
        let subject = subjects::agent::fetch_guild_channels(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_guild_channels requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_guild_channels: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchGuildChannelsCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_guild_channels: failed to deserialize: {}", e);
                    continue;
                }
            };
            let guild = GuildId::new(cmd.guild_id);
            let response_bytes = match http.get_channels(guild).await {
                Ok(channels) => {
                    let fetched: Vec<FetchedChannel> = channels
                        .iter()
                        .map(|ch| FetchedChannel {
                            id: ch.id.get(),
                            channel_type: match ch.kind {
                                SerenityChannelType::Text => ChannelType::GuildText,
                                SerenityChannelType::Voice => ChannelType::GuildVoice,
                                SerenityChannelType::Category => ChannelType::GuildCategory,
                                SerenityChannelType::News => ChannelType::GuildNews,
                                SerenityChannelType::Stage => ChannelType::GuildStageVoice,
                                SerenityChannelType::Forum => ChannelType::GuildForum,
                                _ => ChannelType::Unknown,
                            },
                            guild_id: Some(ch.guild_id.get()),
                            name: Some(ch.name.clone()),
                            topic: ch.topic.clone(),
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_guild_channels: Discord API error for guild {}: {}",
                        cmd.guild_id, e
                    );
                    serde_json::to_vec::<Vec<FetchedChannel>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_guild_channels: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_pinned(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_pinned(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_pinned requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_pinned: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchPinnedMessagesCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_pinned: failed to deserialize: {}", e);
                    continue;
                }
            };
            let channel = ChannelId::new(cmd.channel_id);
            let response_bytes = match http.get_pins(channel).await {
                Ok(messages) => {
                    let fetched: Vec<FetchedMessage> =
                        messages.iter().map(to_fetched_message).collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_pinned: Discord API error for channel {}: {}",
                        cmd.channel_id, e
                    );
                    serde_json::to_vec::<Vec<FetchedMessage>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_pinned: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_roles(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_roles(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_roles requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_roles: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchRolesCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_roles: failed to deserialize: {}", e);
                    continue;
                }
            };
            let guild = GuildId::new(cmd.guild_id);
            let response_bytes = match http.get_guild_roles(guild).await {
                Ok(roles) => {
                    let fetched: Vec<FetchedRole> = roles
                        .iter()
                        .map(|r| FetchedRole {
                            id: r.id.get(),
                            name: r.name.clone(),
                            color: r.colour.0,
                            hoist: r.hoist,
                            mentionable: r.mentionable,
                            permissions: r.permissions.bits(),
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_roles: Discord API error for guild {}: {}",
                        cmd.guild_id, e
                    );
                    serde_json::to_vec::<Vec<FetchedRole>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_roles: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_edit_interaction_response(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::interaction_edit_response(&prefix);
        let mut stream = subscriber
            .subscribe::<EditInteractionResponseCommand>(&subject)
            .await?;
        info!(
            "Listening for interaction_edit_response commands on {}",
            subject
        );
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let mut builder = EditInteractionResponse::new();
                    if let Some(ref content) = cmd.content {
                        builder = builder.content(content);
                    }
                    if !cmd.embeds.is_empty() {
                        let embeds: Vec<CreateEmbed> = cmd.embeds.iter().map(build_embed).collect();
                        builder = builder.embeds(embeds);
                    }
                    if let Err(e) = http
                        .edit_original_interaction_response(
                            &cmd.interaction_token,
                            &builder,
                            vec![],
                        )
                        .await
                    {
                        warn!(
                            "Failed to edit interaction response for token {}: {}",
                            cmd.interaction_token, e
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to deserialize interaction_edit_response command: {}",
                    e
                ),
            }
        }
        Ok(())
    }

    async fn handle_delete_interaction_response(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::interaction_delete_response(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteInteractionResponseCommand>(&subject)
            .await?;
        info!(
            "Listening for interaction_delete_response commands on {}",
            subject
        );
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    if let Err(e) = http
                        .delete_original_interaction_response(&cmd.interaction_token)
                        .await
                    {
                        warn!(
                            "Failed to delete interaction response for token {}: {}",
                            cmd.interaction_token, e
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to deserialize interaction_delete_response command: {}",
                    e
                ),
            }
        }
        Ok(())
    }

    async fn handle_set_channel_permissions(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::channel_set_permissions(&prefix);
        let mut stream = subscriber
            .subscribe::<SetChannelPermissionsCommand>(&subject)
            .await?;
        info!(
            "Listening for channel_set_permissions commands on {}",
            subject
        );
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let target_id = TargetId::new(cmd.target_id);
                    let kind = if cmd.target_type == "member" {
                        PermissionOverwriteType::Member(UserId::new(cmd.target_id))
                    } else {
                        PermissionOverwriteType::Role(RoleId::new(cmd.target_id))
                    };
                    let overwrite = PermissionOverwrite {
                        allow: Permissions::from_bits_truncate(cmd.allow),
                        deny: Permissions::from_bits_truncate(cmd.deny),
                        kind,
                    };
                    if let Err(e) = http
                        .create_permission(channel, target_id, &overwrite, None)
                        .await
                    {
                        warn!(
                            "Failed to set permissions on channel {}: {}",
                            cmd.channel_id, e
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to deserialize channel_set_permissions command: {}",
                    e
                ),
            }
        }
        Ok(())
    }

    async fn handle_delete_channel_permissions(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::channel_delete_permissions(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteChannelPermissionsCommand>(&subject)
            .await?;
        info!(
            "Listening for channel_delete_permissions commands on {}",
            subject
        );
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let channel = ChannelId::new(cmd.channel_id);
                    let target_id = TargetId::new(cmd.target_id);
                    if let Err(e) = http.delete_permission(channel, target_id, None).await {
                        warn!(
                            "Failed to delete permissions on channel {}: {}",
                            cmd.channel_id, e
                        );
                    }
                }
                Err(e) => warn!(
                    "Failed to deserialize channel_delete_permissions command: {}",
                    e
                ),
            }
        }
        Ok(())
    }

    async fn handle_prune_members(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::prune_members(&prefix);
        let mut stream = subscriber
            .subscribe::<PruneMembersCommand>(&subject)
            .await?;
        info!("Listening for prune_members commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    if let Err(e) = http.start_guild_prune(guild, cmd.days, None).await {
                        warn!("Failed to prune members in guild {}: {}", cmd.guild_id, e);
                    }
                }
                Err(e) => warn!("Failed to deserialize prune_members command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_fetch_audit_log(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_audit_log(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_audit_log requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_audit_log: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchAuditLogCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_audit_log: failed to deserialize: {}", e);
                    continue;
                }
            };
            let guild = GuildId::new(cmd.guild_id);
            let before = cmd.before_id.map(serenity::model::id::AuditLogEntryId::new);
            let user = cmd.user_id.map(UserId::new);
            let response_bytes = match http
                .get_audit_logs(guild, None, user, before, cmd.limit)
                .await
            {
                Ok(logs) => {
                    let entries: Vec<AuditLogEntryInfo> = logs
                        .entries
                        .iter()
                        .map(|e| AuditLogEntryInfo {
                            id: e.id.get(),
                            user_id: e.user_id.get(),
                            target_id: e.target_id.map(|t| t.get()),
                            action_type: e.action.num() as u32,
                            reason: e.reason.clone(),
                        })
                        .collect();
                    serde_json::to_vec(&entries).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_audit_log: Discord API error for guild {}: {}",
                        cmd.guild_id, e
                    );
                    serde_json::to_vec::<Vec<AuditLogEntryInfo>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_audit_log: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_scheduled_event_users(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_scheduled_event_users(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!(
            "Listening for fetch_scheduled_event_users requests on {}",
            subject
        );
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_scheduled_event_users: received message without reply subject");
                    continue;
                }
            };
            let cmd: FetchScheduledEventUsersCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_scheduled_event_users: failed to deserialize: {}", e);
                    continue;
                }
            };
            let guild = GuildId::new(cmd.guild_id);
            let event_id = serenity::model::id::ScheduledEventId::new(cmd.event_id);
            let response_bytes = match http
                .get_scheduled_event_users(guild, event_id, cmd.limit, None, None)
                .await
            {
                Ok(users) => {
                    let fetched: Vec<ScheduledEventUserInfo> = users
                        .iter()
                        .map(|u| ScheduledEventUserInfo {
                            event_id: cmd.event_id,
                            user: DiscordUser {
                                id: u.user.id.get(),
                                username: u.user.name.clone(),
                                global_name: u.user.global_name.as_deref().map(String::from),
                                bot: u.user.bot,
                            },
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!("fetch_scheduled_event_users: Discord API error: {}", e);
                    serde_json::to_vec::<Vec<ScheduledEventUserInfo>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_scheduled_event_users: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_edit_sticker(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::model::id::StickerId;
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::sticker_edit(&prefix);
        let mut stream = subscriber.subscribe::<EditStickerCommand>(&subject).await?;
        info!("Listening for sticker_edit commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let sticker_id = StickerId::new(cmd.sticker_id);
                    let mut map = serde_json::Map::new();
                    if let Some(ref name) = cmd.name {
                        map.insert("name".to_string(), serde_json::json!(name));
                    }
                    if let Some(ref desc) = cmd.description {
                        map.insert("description".to_string(), serde_json::json!(desc));
                    }
                    let value = serde_json::Value::Object(map);
                    if let Err(e) = http.edit_sticker(guild, sticker_id, &value, None).await {
                        warn!(
                            "Failed to edit sticker {} in guild {}: {}",
                            cmd.sticker_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize sticker_edit command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_delete_sticker(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::model::id::StickerId;
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::sticker_delete(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteStickerCommand>(&subject)
            .await?;
        info!("Listening for sticker_delete commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let sticker_id = StickerId::new(cmd.sticker_id);
                    if let Err(e) = http.delete_sticker(guild, sticker_id, None).await {
                        warn!(
                            "Failed to delete sticker {} in guild {}: {}",
                            cmd.sticker_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize sticker_delete command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_delete_integration(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::model::id::IntegrationId;
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::integration_delete(&prefix);
        let mut stream = subscriber
            .subscribe::<DeleteIntegrationCommand>(&subject)
            .await?;
        info!("Listening for integration_delete commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let integration_id = IntegrationId::new(cmd.integration_id);
                    if let Err(e) = http
                        .delete_guild_integration(guild, integration_id, None)
                        .await
                    {
                        warn!(
                            "Failed to delete integration {} in guild {}: {}",
                            cmd.integration_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize integration_delete command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_sync_integration(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::model::id::IntegrationId;
        let subscriber = MessageSubscriber::new(client, &prefix);
        let subject = subjects::agent::integration_sync(&prefix);
        let mut stream = subscriber
            .subscribe::<SyncIntegrationCommand>(&subject)
            .await?;
        info!("Listening for integration_sync commands on {}", subject);
        while let Some(result) = stream.next().await {
            match result {
                Ok(cmd) => {
                    let guild = GuildId::new(cmd.guild_id);
                    let integration_id = IntegrationId::new(cmd.integration_id);
                    if let Err(e) = http.start_integration_sync(guild, integration_id).await {
                        warn!(
                            "Failed to sync integration {} in guild {}: {}",
                            cmd.integration_id, cmd.guild_id, e
                        );
                    }
                }
                Err(e) => warn!("Failed to deserialize integration_sync command: {}", e),
            }
        }
        Ok(())
    }

    async fn handle_fetch_voice_regions(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_voice_regions(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_voice_regions requests on {}", subject);
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_voice_regions: received message without reply subject");
                    continue;
                }
            };
            let response_bytes = match http.get_voice_regions().await {
                Ok(regions) => {
                    let fetched: Vec<VoiceRegionInfo> = regions
                        .iter()
                        .map(|r| VoiceRegionInfo {
                            id: r.id.clone(),
                            name: r.name.clone(),
                            optimal: r.optimal,
                            deprecated: r.deprecated,
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!("fetch_voice_regions: Discord API error: {}", e);
                    serde_json::to_vec::<Vec<VoiceRegionInfo>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_voice_regions: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_fetch_application_info(
        http: Arc<Http>,
        client: N,
        prefix: String,
    ) -> Result<()> {
        use serenity::futures::StreamExt as _;
        let subject = subjects::agent::fetch_application_info(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!(
            "Listening for fetch_application_info requests on {}",
            subject
        );
        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_application_info: received message without reply subject");
                    continue;
                }
            };
            let response_bytes = match http.get_current_application_info().await {
                Ok(info) => {
                    let fetched = AppInfo {
                        id: info.id.get(),
                        name: info.name.clone(),
                        description: info.description.clone(),
                        owner_id: info.owner.as_ref().map(|u| u.id.get()),
                    };
                    serde_json::to_vec(&Some(fetched)).unwrap_or_default()
                }
                Err(e) => {
                    error!("fetch_application_info: Discord API error: {}", e);
                    serde_json::to_vec::<Option<AppInfo>>(&None).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_application_info: failed to send reply: {}", e);
            }
        }
        Ok(())
    }

    async fn handle_pairing_approve(
        http: Arc<Http>,
        client: N,
        prefix: String,
        pairing_state: Arc<PairingState>,
        publisher: MessagePublisher,
    ) -> Result<()> {
        use discord_types::PairingApproveCommand;
        use serenity::futures::StreamExt as _;

        let subject = subjects::agent::pairing_approve(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for pairing_approve on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: PairingApproveCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("pairing_approve: bad payload: {}", e);
                    continue;
                }
            };

            match pairing_state.approve_by_code(&cmd.code) {
                Some((user_id, channel_id)) => {
                    info!(user_id, "Pairing approved for user");

                    // Send DM notification
                    let channel = serenity::model::id::ChannelId::new(channel_id);
                    if let Err(e) = channel
                        .say(
                            &http,
                            "✅ Your account has been approved! You can now send me messages.",
                        )
                        .await
                    {
                        warn!(user_id, "Failed to send pairing approval DM: {}", e);
                    }

                    // Publish event
                    let event = discord_types::events::PairingApprovedEvent {
                        metadata: discord_types::events::EventMetadata::new(
                            format!("dm:{}:{}", user_id, user_id),
                            0,
                        ),
                        user_id,
                    };
                    let event_subject = subjects::bot::pairing_approved(&prefix);
                    if let Err(e) = publisher.publish(&event_subject, &event).await {
                        warn!("Failed to publish pairing_approved: {}", e);
                    }
                }
                None => {
                    warn!(code = %cmd.code, "pairing_approve: code not found");
                }
            }
        }

        Ok(())
    }

    async fn handle_pairing_reject(
        http: Arc<Http>,
        client: N,
        prefix: String,
        pairing_state: Arc<PairingState>,
        publisher: MessagePublisher,
    ) -> Result<()> {
        use discord_types::PairingRejectCommand;
        use serenity::futures::StreamExt as _;

        let subject = subjects::agent::pairing_reject(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for pairing_reject on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: PairingRejectCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("pairing_reject: bad payload: {}", e);
                    continue;
                }
            };

            match pairing_state.reject_by_code(&cmd.code) {
                Some((user_id, channel_id)) => {
                    info!(user_id, "Pairing rejected for user");

                    // Send DM notification
                    let channel = serenity::model::id::ChannelId::new(channel_id);
                    if let Err(e) = channel
                        .say(&http, "❌ Your pairing request was rejected.")
                        .await
                    {
                        warn!(user_id, "Failed to send pairing rejection DM: {}", e);
                    }

                    // Publish event
                    let event = discord_types::events::PairingRejectedEvent {
                        metadata: discord_types::events::EventMetadata::new(
                            format!("dm:{}:{}", user_id, user_id),
                            0,
                        ),
                        user_id,
                    };
                    let event_subject = subjects::bot::pairing_rejected(&prefix);
                    if let Err(e) = publisher.publish(&event_subject, &event).await {
                        warn!("Failed to publish pairing_rejected: {}", e);
                    }
                }
                None => {
                    warn!(code = %cmd.code, "pairing_reject: code not found");
                }
            }
        }

        Ok(())
    }

    async fn handle_create_poll(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;

        let subject = subjects::agent::create_poll(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for create_poll on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: CreatePollCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("create_poll: bad payload: {}", e);
                    continue;
                }
            };

            let channel_id = ChannelId::new(cmd.channel_id);
            let answers: Vec<CreatePollAnswer> = cmd
                .answers
                .iter()
                .map(|a| CreatePollAnswer::new().text(a))
                .collect();
            let mut poll = CreatePoll::new()
                .question(cmd.question.as_str())
                .answers(answers)
                .duration(std::time::Duration::from_secs(
                    cmd.duration_hours as u64 * 3600,
                ));
            if cmd.allow_multiselect {
                poll = poll.allow_multiselect();
            }
            let builder = CreateMessage::new().poll(poll);
            if let Err(e) = channel_id.send_message(&*http, builder).await {
                warn!("create_poll failed for channel {}: {}", cmd.channel_id, e);
            }
        }

        Ok(())
    }

    async fn handle_create_sticker(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;

        let subject = subjects::agent::create_sticker(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for create_sticker on {}", subject);

        while let Some(msg) = sub.next().await {
            let cmd: CreateStickerCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("create_sticker: bad payload: {}", e);
                    continue;
                }
            };

            let guild = GuildId::new(cmd.guild_id);
            let file = match CreateAttachment::path(&cmd.file_path).await {
                Ok(f) => f,
                Err(e) => {
                    warn!("create_sticker: failed to load '{}': {}", cmd.file_path, e);
                    continue;
                }
            };
            let sticker = CreateSticker::new(&cmd.name, file)
                .description(&cmd.description)
                .tags(&cmd.tags);
            if let Err(e) = guild.create_sticker(&*http, sticker).await {
                warn!("create_sticker failed in guild {}: {}", cmd.guild_id, e);
            }
        }

        Ok(())
    }

    async fn handle_fetch_emojis(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;

        let subject = subjects::agent::fetch_emojis(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_emojis requests on {}", subject);

        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_emojis: message without reply subject");
                    continue;
                }
            };
            let cmd: FetchEmojisCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_emojis: bad payload: {}", e);
                    continue;
                }
            };
            let guild = GuildId::new(cmd.guild_id);
            let response_bytes = match guild.emojis(&*http).await {
                Ok(emojis) => {
                    let fetched: Vec<Emoji> = emojis
                        .iter()
                        .map(|e| Emoji {
                            id: Some(e.id.get()),
                            name: e.name.clone(),
                            animated: e.animated,
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_emojis: Discord API error for guild {}: {}",
                        cmd.guild_id, e
                    );
                    serde_json::to_vec::<Vec<Emoji>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_emojis: failed to publish reply: {}", e);
            }
        }

        Ok(())
    }

    async fn handle_fetch_bans(http: Arc<Http>, client: N, prefix: String) -> Result<()> {
        use serenity::futures::StreamExt as _;

        let subject = subjects::agent::fetch_bans(&prefix);
        let mut sub = client.subscribe(subject.clone()).await?;
        info!("Listening for fetch_bans requests on {}", subject);

        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("fetch_bans: message without reply subject");
                    continue;
                }
            };
            let cmd: FetchBansCommand = match serde_json::from_slice(&msg.payload) {
                Ok(c) => c,
                Err(e) => {
                    warn!("fetch_bans: bad payload: {}", e);
                    continue;
                }
            };
            let guild = GuildId::new(cmd.guild_id);
            let response_bytes = match guild.bans(&*http, None, None).await {
                Ok(bans) => {
                    let fetched: Vec<FetchedBan> = bans
                        .iter()
                        .map(|b| FetchedBan {
                            user_id: b.user.id.get(),
                            username: b.user.name.clone(),
                            reason: b.reason.clone(),
                        })
                        .collect();
                    serde_json::to_vec(&fetched).unwrap_or_default()
                }
                Err(e) => {
                    error!(
                        "fetch_bans: Discord API error for guild {}: {}",
                        cmd.guild_id, e
                    );
                    serde_json::to_vec::<Vec<FetchedBan>>(&vec![]).unwrap_or_default()
                }
            };
            if let Err(e) = client
                .publish_with_headers(reply, async_nats::HeaderMap::new(), response_bytes.into())
                .await
            {
                error!("fetch_bans: failed to publish reply: {}", e);
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

#[cfg(test)]
mod pure_fn_tests {
    use super::*;

    // ── truncate ──────────────────────────────────────────────────────────────

    #[test]
    fn test_truncate_short_is_unchanged() {
        assert_eq!(truncate("hello"), "hello");
    }

    #[test]
    fn test_truncate_exactly_max_is_unchanged() {
        let s = "a".repeat(MAX_DISCORD_LEN);
        assert_eq!(truncate(&s).len(), MAX_DISCORD_LEN);
    }

    #[test]
    fn test_truncate_over_max_is_cut() {
        let s = "b".repeat(MAX_DISCORD_LEN + 50);
        let t = truncate(&s);
        assert!(t.len() <= MAX_DISCORD_LEN);
    }

    #[test]
    fn test_truncate_multibyte_respects_char_boundary() {
        let s = "€".repeat(700); // 700 × 3 bytes = 2100 bytes
        let t = truncate(&s);
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
        assert!(t.len() <= MAX_DISCORD_LEN);
    }

    // ── chunk_text ────────────────────────────────────────────────────────────

    #[test]
    fn test_chunk_text_short_text_is_one_chunk() {
        let chunks = chunk_text("hello world", 2000, None);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn test_chunk_text_empty_gives_empty_chunk() {
        let chunks = chunk_text("", 2000, None);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[test]
    fn test_chunk_text_splits_on_line_limit() {
        let text = "line1\nline2\nline3\nline4";
        let chunks = chunk_text(text, 2000, Some(2));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "line1\nline2");
        assert_eq!(chunks[1], "line3\nline4");
    }

    #[test]
    fn test_chunk_text_no_line_limit_all_in_one() {
        let text = "a\nb\nc\nd\ne";
        let chunks = chunk_text(text, 2000, None);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_chunk_text_line_limit_exact_multiple() {
        let chunks = chunk_text("a\nb\nc\nd", 2000, Some(2));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "a\nb");
        assert_eq!(chunks[1], "c\nd");
    }

    #[test]
    fn test_chunk_text_splits_at_char_limit() {
        let text = "hello world foo bar baz!!";
        let chunks = chunk_text(text, 10, None);
        for chunk in &chunks {
            assert!(chunk.len() <= 10, "chunk too long: {:?}", chunk);
        }
    }

    #[test]
    fn test_chunk_text_hard_break_when_no_whitespace() {
        let text = "a".repeat(50);
        let chunks = chunk_text(&text, 20, None);
        for chunk in &chunks {
            assert!(chunk.len() <= 20);
        }
        assert_eq!(chunks.join(""), text);
    }

    #[test]
    fn test_chunk_text_single_long_line_multiple_chunks() {
        let text = "word ".repeat(500); // 2500 chars
        let chunks = chunk_text(text.trim(), 100, None);
        for chunk in &chunks {
            assert!(chunk.len() <= 100, "chunk too long: {}", chunk.len());
        }
    }

    // ── extract_reply_tag ─────────────────────────────────────────────────────

    #[test]
    fn test_extract_no_tag_returns_original() {
        let (content, id) = extract_reply_tag("hello world");
        assert_eq!(content, "hello world");
        assert_eq!(id, None);
    }

    #[test]
    fn test_extract_reply_tag_at_start() {
        let (content, id) = extract_reply_tag("[[reply_to:123]] hello");
        assert_eq!(id, Some(123));
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_extract_reply_tag_in_middle() {
        let (content, id) = extract_reply_tag("hi [[reply_to:456]] there");
        assert_eq!(id, Some(456));
        assert!(!content.contains("[[reply_to:"), "tag must be stripped");
    }

    #[test]
    fn test_extract_reply_tag_invalid_id_strips_tag() {
        let (content, id) = extract_reply_tag("[[reply_to:abc]] hello");
        assert_eq!(id, None);
        assert!(
            !content.contains("[[reply_to:abc]]"),
            "tag must be stripped"
        );
    }

    #[test]
    fn test_extract_reply_to_current_stripped() {
        let (content, id) = extract_reply_tag("[[reply_to_current]] hello");
        assert_eq!(id, None);
        assert!(!content.contains("[[reply_to_current]]"));
        assert!(content.contains("hello"));
    }

    #[test]
    fn test_extract_reply_tag_zero_id() {
        let (_, id) = extract_reply_tag("[[reply_to:0]] msg");
        assert_eq!(id, Some(0));
    }

    #[test]
    fn test_extract_reply_tag_large_id() {
        let (_, id) = extract_reply_tag("[[reply_to:1234567890123456789]] msg");
        assert_eq!(id, Some(1234567890123456789));
    }

    #[test]
    fn test_extract_no_tag_no_mutation() {
        let input = "just a plain message with no tags at all";
        let (content, id) = extract_reply_tag(input);
        assert_eq!(content, input);
        assert_eq!(id, None);
    }

    // ── parse_reaction_type ───────────────────────────────────────────────────

    #[test]
    fn test_parse_unicode_emoji() {
        let rt = parse_reaction_type("👍");
        assert!(matches!(rt, serenity::model::channel::ReactionType::Unicode(s) if s == "👍"));
    }

    #[test]
    fn test_parse_custom_emoji_static() {
        let rt = parse_reaction_type("wave:123456789");
        match rt {
            serenity::model::channel::ReactionType::Custom { animated, id, name } => {
                assert!(!animated);
                assert_eq!(id.get(), 123456789);
                assert_eq!(name.as_deref(), Some("wave"));
            }
            _ => panic!("expected Custom reaction"),
        }
    }

    #[test]
    fn test_parse_custom_emoji_animated_flag_1() {
        let rt = parse_reaction_type("dance:987654321:1");
        match rt {
            serenity::model::channel::ReactionType::Custom { animated, id, name } => {
                assert!(animated);
                assert_eq!(id.get(), 987654321);
                assert_eq!(name.as_deref(), Some("dance"));
            }
            _ => panic!("expected Custom reaction"),
        }
    }

    #[test]
    fn test_parse_custom_emoji_animated_flag_word() {
        let rt = parse_reaction_type("spin:111222333:animated");
        match rt {
            serenity::model::channel::ReactionType::Custom { animated, .. } => assert!(animated),
            _ => panic!("expected Custom reaction"),
        }
    }

    #[test]
    fn test_parse_custom_emoji_not_animated_other_flag() {
        let rt = parse_reaction_type("icon:555:false");
        match rt {
            serenity::model::channel::ReactionType::Custom { animated, .. } => assert!(!animated),
            _ => panic!("expected Custom reaction"),
        }
    }

    #[test]
    fn test_parse_custom_emoji_invalid_id_falls_back_to_unicode() {
        let rt = parse_reaction_type("wave:notanumber");
        assert!(matches!(
            rt,
            serenity::model::channel::ReactionType::Unicode(_)
        ));
    }

    #[test]
    fn test_parse_plain_text_is_unicode() {
        let rt = parse_reaction_type(":thumbsup:");
        assert!(matches!(
            rt,
            serenity::model::channel::ReactionType::Unicode(_)
        ));
    }
}
