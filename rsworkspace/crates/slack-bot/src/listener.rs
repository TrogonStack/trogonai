use async_nats::jetstream::Context as JsContext;
use reqwest::Client as HttpClient;
use slack_morphism::prelude::*;
use slack_morphism::errors::SlackClientError;
use slack_nats::publisher::{
    publish_app_home, publish_block_action, publish_channel, publish_inbound, publish_member,
    publish_message_changed, publish_message_deleted, publish_pin, publish_reaction,
    publish_slash_command, publish_thread_broadcast, publish_view_closed, publish_view_submission,
};
use slack_types::events::{
    ChannelEventKind, PinEventKind, SessionType, SlackAppHomeOpenedEvent, SlackAttachment,
    SlackBlockActionEvent, SlackChannelEvent, SlackFile as OurSlackFile, SlackInboundMessage,
    SlackMemberEvent, SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackPinEvent,
    SlackReactionEvent, SlackSlashCommandEvent, SlackThreadBroadcastEvent, SlackViewClosedEvent,
    SlackViewSubmissionEvent,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

/// Maximum image size that will be base64-encoded and forwarded to the agent.
/// Matches the practical limit for Claude's vision API.
const MAX_IMAGE_BYTES: u64 = 5_000_000;

/// Shared state injected into the Socket Mode listener via `with_user_state`.
#[derive(Clone)]
pub struct BotState {
    pub nats: Arc<JsContext>,
    /// When set, messages whose `user` field matches are dropped before
    /// publishing to NATS. This prevents the bot from reacting to its own
    /// messages when the Slack app is configured to receive all messages.
    pub bot_user_id: Option<String>,
    /// When true, channel/group messages require an @mention or be a thread
    /// reply before being published to NATS. Mirrors OpenClaw `requireMention`.
    pub mention_gating: bool,
    /// Channels where mention gating is always ON regardless of `mention_gating`.
    pub mention_gating_channels: HashSet<String>,
    /// Channels where mention gating is always OFF regardless of `mention_gating`.
    pub no_mention_channels: HashSet<String>,
    /// Custom text patterns that activate the bot even when mention gating is enabled.
    pub mention_patterns: Vec<String>,
    /// When true, messages from bots are forwarded to NATS instead of being
    /// silently dropped.
    pub allow_bots: bool,
    /// Bot token used to authenticate file downloads from Slack's CDN.
    pub bot_token: String,
    /// HTTP client reused across file download requests.
    pub http_client: reqwest::Client,
    /// Maximum file size in MB for inbound media downloads.
    pub media_max_mb: u64,
    /// Optional user token (xoxp-...) for API calls requiring user-level permissions.
    /// When set, prefer this over bot_token for users.info and emoji.list calls.
    #[allow(dead_code)]
    pub user_token: Option<String>,
}

/// Module-level display name cache shared across all Socket Mode callbacks.
static USER_DISPLAY_NAME_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Module-level cache for the workspace's custom emoji map (name → URL/alias).
/// Populated lazily on the first message and shared across all callbacks.
static EMOJI_CACHE: LazyLock<Mutex<Option<HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(None));

pub async fn handle_push_event(
    event: SlackPushEventCallback,
    _client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = {
        let states = states.read().await;
        states
            .get_user_state::<BotState>()
            .expect("BotState must be in user state")
            .clone()
    };

    match event.event {
        SlackEventCallbackBody::Message(msg) => {
            // Drop messages from any bot (including ourselves) unless allow_bots is set.
            if msg.sender.bot_id.is_some() && !state.allow_bots {
                return Ok(());
            }

            match &msg.subtype {
                Some(SlackMessageEventType::MessageChanged) => {
                    let channel = msg
                        .origin
                        .channel
                        .as_ref()
                        .map(|c| c.0.clone())
                        .unwrap_or_default();
                    let ts = msg.origin.ts.0.clone();
                    let event_ts = Some(msg.origin.ts.0.clone());
                    let previous_text = msg
                        .previous_message
                        .as_ref()
                        .and_then(|m| m.content.as_ref())
                        .and_then(|c| c.text.clone());
                    let new_text = msg
                        .message
                        .as_ref()
                        .and_then(|m| m.content.as_ref())
                        .and_then(|c| c.text.clone());
                    let thread_ts = msg.origin.thread_ts.as_ref().map(|t| t.0.clone());
                    let user = msg
                        .message
                        .as_ref()
                        .and_then(|m| m.sender.user.as_ref())
                        .map(|u| u.0.clone());

                    let ev = SlackMessageChangedEvent {
                        channel,
                        ts,
                        event_ts,
                        previous_text,
                        new_text,
                        thread_ts,
                        user,
                    };

                    if let Err(e) = publish_message_changed(&state.nats, &ev).await {
                        tracing::error!(error = %e, "Failed to publish message_changed to NATS");
                    }
                }

                Some(SlackMessageEventType::MessageDeleted) => {
                    let channel = msg
                        .origin
                        .channel
                        .as_ref()
                        .map(|c| c.0.clone())
                        .unwrap_or_default();
                    let deleted_ts = msg
                        .deleted_ts
                        .as_ref()
                        .map(|t| t.0.clone())
                        .unwrap_or_else(|| msg.origin.ts.0.clone());
                    let event_ts = Some(msg.origin.ts.0.clone());
                    let thread_ts = msg.origin.thread_ts.as_ref().map(|t| t.0.clone());

                    let ev = SlackMessageDeletedEvent {
                        channel,
                        deleted_ts,
                        event_ts,
                        thread_ts,
                    };

                    if let Err(e) = publish_message_deleted(&state.nats, &ev).await {
                        tracing::error!(error = %e, "Failed to publish message_deleted to NATS");
                    }
                }

                Some(SlackMessageEventType::ThreadBroadcast) => {
                    let channel = msg
                        .origin
                        .channel
                        .as_ref()
                        .map(|c| c.0.clone())
                        .unwrap_or_default();
                    let user = msg
                        .sender
                        .user
                        .as_ref()
                        .map(|u| u.0.clone())
                        .unwrap_or_default();
                    let text = msg
                        .content
                        .as_ref()
                        .and_then(|c| c.text.as_deref())
                        .unwrap_or("")
                        .to_string();
                    let ts = msg.origin.ts.0.clone();
                    let thread_ts = msg
                        .origin
                        .thread_ts
                        .as_ref()
                        .map(|t| t.0.clone())
                        .unwrap_or_else(|| msg.origin.ts.0.clone());
                    let event_ts = None;

                    let ev = SlackThreadBroadcastEvent {
                        channel,
                        user,
                        text,
                        ts,
                        thread_ts,
                        event_ts,
                    };

                    if let Err(e) = publish_thread_broadcast(&state.nats, &ev).await {
                        tracing::error!(error = %e, "Failed to publish thread_broadcast to NATS");
                    }
                }

                Some(_) => {
                    // Other subtypes (channel_join, bot_message, etc.) — ignore.
                }

                None => {
                    // Regular user message.
                    let user = msg
                        .sender
                        .user
                        .as_ref()
                        .map(|u| u.0.clone())
                        .unwrap_or_default();

                    // Drop messages originating from our own bot user.
                    if let Some(ref bot_uid) = state.bot_user_id
                        && &user == bot_uid
                    {
                        return Ok(());
                    }

                    let channel_id = msg
                        .origin
                        .channel
                        .as_ref()
                        .map(|c| c.0.clone())
                        .unwrap_or_default();

                    let raw_text = msg
                        .content
                        .as_ref()
                        .and_then(|c| c.text.as_deref())
                        .unwrap_or("")
                        .to_string();
                    let text = strip_bot_mention(&raw_text, state.bot_user_id.as_deref());

                    if text.is_empty() {
                        return Ok(());
                    }
                    let custom_emoji = fetch_custom_emoji(&state.bot_token, &state.http_client).await;
                    let text = annotate_custom_emoji(&text, &custom_emoji);

                    let ts = msg.origin.ts.0.clone();
                    let thread_ts = msg.origin.thread_ts.as_ref().map(|t| t.0.clone());

                    let session_type =
                        match msg.origin.channel_type.as_ref().map(|ct| ct.0.as_str()) {
                            Some("im") => SessionType::Direct,
                            Some("mpim") => SessionType::Group,
                            _ => SessionType::Channel,
                        };

                    // Mention-gating (OpenClaw: requireMention = true by default).
                    // Per-channel overrides take precedence over the global flag.
                    if resolve_mention_gating(
                        &session_type,
                        &channel_id,
                        thread_ts.as_deref(),
                        state.mention_gating,
                        &state.mention_gating_channels,
                        &state.no_mention_channels,
                        &text,
                        &state.mention_patterns,
                    ) {
                        match &state.bot_user_id {
                            Some(bot_uid) => {
                                // Gate on raw_text: the stripped `text` no longer
                                // contains the mention so we must check the original.
                                if !raw_text.contains(&format!("<@{}>", bot_uid)) {
                                    return Ok(());
                                }
                            }
                            None => {
                                // bot_user_id not configured — cannot gate, allow all.
                            }
                        }
                    }

                    // Compute session key for conversation routing.
                    let session_key = compute_session_key(
                        &session_type,
                        &channel_id,
                        &user,
                        thread_ts.as_deref(),
                    );

                    tracing::debug!(
                        channel = %channel_id,
                        user = %user,
                        session_type = ?session_type,
                        session_key = ?session_key,
                        "Received message, publishing to NATS"
                    );

                    let raw_files = extract_files(msg.content.as_ref());
                    let files =
                        fetch_file_contents(raw_files, &state.bot_token, &state.http_client, state.media_max_mb).await;
                    let attachments = extract_attachments(msg.content.as_ref());
                    let display_name =
                        lookup_display_name(&user, &state.bot_token, &state.http_client)
                            .await;

                    let inbound = SlackInboundMessage {
                        channel: channel_id,
                        user,
                        text,
                        ts,
                        event_ts: None,
                        thread_ts,
                        parent_user_id: None,
                        session_type,
                        source: None,
                        session_key,
                        files,
                        attachments,
                        display_name,
                    };

                    if let Err(e) = publish_inbound(&state.nats, &inbound).await {
                        tracing::error!(error = %e, "Failed to publish inbound message to NATS");
                    }
                }
            }
        }

        SlackEventCallbackBody::AppMention(mention) => {
            let channel_id = mention.channel.0.clone();
            let user = mention.user.0.clone();
            let raw_text = mention.content.text.as_deref().unwrap_or("").to_string();
            let text = strip_bot_mention(&raw_text, state.bot_user_id.as_deref());

            if text.is_empty() {
                return Ok(());
            }
            let custom_emoji = fetch_custom_emoji(&state.bot_token, &state.http_client).await;
            let text = annotate_custom_emoji(&text, &custom_emoji);

            let ts = mention.origin.ts.0.clone();
            let thread_ts = mention.origin.thread_ts.as_ref().map(|t| t.0.clone());

            let session_key = compute_session_key(
                &SessionType::Channel,
                &channel_id,
                &user,
                thread_ts.as_deref(),
            );

            let raw_files = extract_files(Some(&mention.content));
            let files =
                fetch_file_contents(raw_files, &state.bot_token, &state.http_client, state.media_max_mb).await;
            let display_name =
                lookup_display_name(&user, &state.bot_token, &state.http_client)
                    .await;

            tracing::debug!(
                channel = %channel_id,
                user = %user,
                session_key = ?session_key,
                "Received app_mention, publishing to NATS"
            );

            let inbound = SlackInboundMessage {
                channel: channel_id,
                user,
                text,
                ts,
                event_ts: None,
                thread_ts,
                parent_user_id: None,
                session_type: SessionType::Channel,
                source: Some("app_mention".to_string()),
                session_key,
                files,
                attachments: vec![],
                display_name,
            };

            if let Err(e) = publish_inbound(&state.nats, &inbound).await {
                tracing::error!(error = %e, "Failed to publish app_mention to NATS");
            }
        }

        SlackEventCallbackBody::ReactionAdded(ev) => {
            let channel = match &ev.item {
                SlackReactionsItem::Message(m) => m.origin.channel.as_ref().map(|c| c.0.clone()),
                _ => None,
            };
            let item_ts = match &ev.item {
                SlackReactionsItem::Message(m) => Some(m.origin.ts.0.clone()),
                _ => None,
            };

            let reaction_ev = SlackReactionEvent {
                reaction: ev.reaction.0.clone(),
                user: ev.user.0.clone(),
                channel,
                item_ts,
                item_user: ev.item_user.as_ref().map(|u| u.0.clone()),
                event_ts: ev.event_ts.0.clone(),
                added: true,
            };

            if let Err(e) = publish_reaction(&state.nats, &reaction_ev).await {
                tracing::error!(error = %e, "Failed to publish reaction_added to NATS");
            }
        }

        SlackEventCallbackBody::ReactionRemoved(ev) => {
            let channel = match &ev.item {
                SlackReactionsItem::Message(m) => m.origin.channel.as_ref().map(|c| c.0.clone()),
                _ => None,
            };
            let item_ts = match &ev.item {
                SlackReactionsItem::Message(m) => Some(m.origin.ts.0.clone()),
                _ => None,
            };

            let reaction_ev = SlackReactionEvent {
                reaction: ev.reaction.0.clone(),
                user: ev.user.0.clone(),
                channel,
                item_ts,
                item_user: ev.item_user.as_ref().map(|u| u.0.clone()),
                event_ts: ev.event_ts.0.clone(),
                added: false,
            };

            if let Err(e) = publish_reaction(&state.nats, &reaction_ev).await {
                tracing::error!(error = %e, "Failed to publish reaction_removed to NATS");
            }
        }

        SlackEventCallbackBody::MemberJoinedChannel(ev) => {
            let member_ev = SlackMemberEvent {
                user: ev.user.0.clone(),
                channel: ev.channel.0.clone(),
                channel_type: Some(ev.channel_type.0.clone()),
                team: Some(ev.team.0.clone()),
                inviter: ev.inviter.as_ref().map(|u| u.0.clone()),
                joined: true,
            };

            if let Err(e) = publish_member(&state.nats, &member_ev).await {
                tracing::error!(error = %e, "Failed to publish member_joined to NATS");
            }
        }

        SlackEventCallbackBody::MemberLeftChannel(ev) => {
            let member_ev = SlackMemberEvent {
                user: ev.user.0.clone(),
                channel: ev.channel.0.clone(),
                channel_type: Some(ev.channel_type.0.clone()),
                team: Some(ev.team.0.clone()),
                inviter: None,
                joined: false,
            };

            if let Err(e) = publish_member(&state.nats, &member_ev).await {
                tracing::error!(error = %e, "Failed to publish member_left to NATS");
            }
        }

        SlackEventCallbackBody::ChannelCreated(ev) => {
            let channel_ev = SlackChannelEvent {
                kind: ChannelEventKind::Created,
                channel_id: ev.channel.id.0.clone(),
                channel_name: ev.channel.name.clone(),
                user: ev.channel.creator.as_ref().map(|u| u.0.clone()),
            };

            if let Err(e) = publish_channel(&state.nats, &channel_ev).await {
                tracing::error!(error = %e, "Failed to publish channel_created to NATS");
            }
        }

        SlackEventCallbackBody::ChannelDeleted(ev) => {
            let channel_ev = SlackChannelEvent {
                kind: ChannelEventKind::Deleted,
                channel_id: ev.channel.0.clone(),
                channel_name: None,
                user: None,
            };

            if let Err(e) = publish_channel(&state.nats, &channel_ev).await {
                tracing::error!(error = %e, "Failed to publish channel_deleted to NATS");
            }
        }

        SlackEventCallbackBody::ChannelArchive(ev) => {
            let channel_ev = SlackChannelEvent {
                kind: ChannelEventKind::Archived,
                channel_id: ev.channel.0.clone(),
                channel_name: None,
                user: Some(ev.user.0.clone()),
            };

            if let Err(e) = publish_channel(&state.nats, &channel_ev).await {
                tracing::error!(error = %e, "Failed to publish channel_archive to NATS");
            }
        }

        SlackEventCallbackBody::ChannelRename(ev) => {
            let channel_ev = SlackChannelEvent {
                kind: ChannelEventKind::Renamed,
                channel_id: ev.channel.id.0.clone(),
                channel_name: ev.channel.name.clone(),
                user: None,
            };

            if let Err(e) = publish_channel(&state.nats, &channel_ev).await {
                tracing::error!(error = %e, "Failed to publish channel_rename to NATS");
            }
        }

        SlackEventCallbackBody::ChannelUnarchive(ev) => {
            let channel_ev = SlackChannelEvent {
                kind: ChannelEventKind::Unarchived,
                channel_id: ev.channel.0.clone(),
                channel_name: None,
                user: Some(ev.user.0.clone()),
            };

            if let Err(e) = publish_channel(&state.nats, &channel_ev).await {
                tracing::error!(error = %e, "Failed to publish channel_unarchive to NATS");
            }
        }

        SlackEventCallbackBody::AppHomeOpened(ev) => {
            let app_home_ev = SlackAppHomeOpenedEvent {
                user: ev.user.0.clone(),
                tab: ev.tab.unwrap_or_default(),
                view_id: None,
            };
            if let Err(e) = publish_app_home(&state.nats, &app_home_ev).await {
                tracing::error!(error = %e, "Failed to publish app_home_opened to NATS");
            }
        }

        SlackEventCallbackBody::LinkShared(ev) => {
            let channel = ev.channel.0.clone();
            let message_ts = ev.message_ts.0.clone();
            let urls: Vec<String> = ev.links.iter().map(|l| l.url.to_string()).collect();
            let bot_token = state.bot_token.clone();
            let http_client = state.http_client.clone();
            tokio::spawn(async move {
                unfurl_links(&bot_token, &http_client, &channel, &message_ts, &urls).await;
            });
        }

        _ => {
            // Unhandled event types.  Note: pin_added / pin_removed events never
            // reach this arm because slack-morphism v2.17 fails to deserialise
            // them (SlackEventCallbackBody has no typed variant for those event
            // types), so the parse error is caught in error_handler instead,
            // where the raw JSON body is re-parsed by try_parse_pin_event.
            tracing::debug!("Ignoring unhandled push event type");
        }
    }

    Ok(())
}

pub async fn handle_interaction_event(
    event: SlackInteractionEvent,
    _client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = {
        let states = states.read().await;
        states
            .get_user_state::<BotState>()
            .expect("BotState must be in user state")
            .clone()
    };

    match event {
        SlackInteractionEvent::BlockActions(ev) => {
            let channel_id = ev.channel.as_ref().map(|c| c.id.0.clone());
            let user_id = ev.user.as_ref().map(|u| u.id.0.clone()).unwrap_or_default();
            let message_ts = ev.message.as_ref().map(|m| m.origin.ts.0.clone());
            let trigger_id = Some(ev.trigger_id.0.clone());

            for action in ev.actions.iter().flatten() {
                let block_action_ev = SlackBlockActionEvent {
                    action_id: action.action_id.0.clone(),
                    block_id: action.block_id.as_ref().map(|b| b.0.clone()),
                    user_id: user_id.clone(),
                    channel_id: channel_id.clone(),
                    message_ts: message_ts.clone(),
                    value: action.value.clone(),
                    trigger_id: trigger_id.clone(),
                };

                if let Err(e) = publish_block_action(&state.nats, &block_action_ev).await {
                    tracing::error!(error = %e, "Failed to publish block_action to NATS");
                }
            }
        }

        SlackInteractionEvent::ViewSubmission(ev) => {
            let user_id = ev.user.id.0.clone();
            let trigger_id = ev.trigger_id.map(|t| t.0).unwrap_or_default();
            let view_id = ev.view.state_params.id.0.clone();
            let callback_id = match &ev.view.view {
                SlackView::Modal(m) => m.callback_id.as_ref().map(|c| c.0.clone()),
                SlackView::Home(_) => None,
            };
            let values = serde_json::to_value(&ev.view.state_params.state)
                .unwrap_or(serde_json::Value::Null);

            let view_submission_ev = SlackViewSubmissionEvent {
                user_id,
                trigger_id,
                view_id,
                callback_id,
                values,
            };

            if let Err(e) = publish_view_submission(&state.nats, &view_submission_ev).await {
                tracing::error!(error = %e, "Failed to publish view_submission to NATS");
            }
        }

        SlackInteractionEvent::ViewClosed(ev) => {
            let user_id = ev.user.id.0.clone();
            let trigger_id = ev.trigger_id.map(|t| t.0).unwrap_or_default();
            let view_id = ev.view.state_params.id.0.clone();
            let callback_id = match &ev.view.view {
                SlackView::Modal(m) => m.callback_id.as_ref().map(|c| c.0.clone()),
                SlackView::Home(_) => None,
            };

            let view_closed_ev = SlackViewClosedEvent {
                user_id,
                trigger_id,
                view_id,
                callback_id,
                values: serde_json::Value::Null,
            };

            if let Err(e) = publish_view_closed(&state.nats, &view_closed_ev).await {
                tracing::error!(error = %e, "Failed to publish view_closed to NATS");
            }
        }

        _ => {}
    }

    Ok(())
}

pub async fn handle_command_event(
    event: SlackCommandEvent,
    _client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> Result<SlackCommandEventResponse, Box<dyn std::error::Error + Send + Sync>> {
    let state = {
        let states = states.read().await;
        states
            .get_user_state::<BotState>()
            .expect("BotState must be in user state")
            .clone()
    };

    let slash_ev = SlackSlashCommandEvent {
        command: event.command.0.clone(),
        text: event.text.clone(),
        user_id: event.user_id.0.clone(),
        channel_id: event.channel_id.0.clone(),
        team_id: Some(event.team_id.0.clone()),
        response_url: event.response_url.0.to_string(),
        trigger_id: Some(event.trigger_id.0.clone()),
    };

    if let Err(e) = publish_slash_command(&state.nats, &slash_ev).await {
        tracing::error!(error = %e, "Failed to publish slash command to NATS");
    }

    Ok(SlackCommandEventResponse::new(
        SlackMessageContent::new().with_text("Command received.".into()),
    ))
}

/// Download text content for files that have a text-like MIME type and are
/// within the size limit. Binary or oversized files are passed as metadata only.
async fn fetch_file_contents(
    files: Vec<OurSlackFile>,
    bot_token: &str,
    http_client: &HttpClient,
    media_max_mb: u64,
) -> Vec<OurSlackFile> {
    let mut out = Vec::with_capacity(files.len());
    for mut file in files {
        let is_text = file
            .mimetype
            .as_deref()
            .map(|m| {
                m.starts_with("text/")
                    || matches!(
                        m,
                        "application/json"
                            | "application/xml"
                            | "application/javascript"
                            | "application/x-yaml"
                    )
            })
            .unwrap_or(false);
        let within_limit = file
            .size
            .map(|s| s <= media_max_mb * 1024 * 1024)
            .unwrap_or(true);

        if is_text && within_limit {
            let url = file
                .url_private_download
                .as_deref()
                .or(file.url_private.as_deref());
            if let Some(url) = url {
                match http_client.get(url).bearer_auth(bot_token).send().await {
                    Ok(resp) if resp.status().is_success() => match resp.text().await {
                        Ok(text) => file.content = Some(text),
                        Err(e) => tracing::warn!(error = %e, "Failed to read file response body"),
                    },
                    Ok(resp) => tracing::warn!(
                        status = %resp.status(),
                        "Non-success status downloading file"
                    ),
                    Err(e) => tracing::warn!(error = %e, "HTTP error downloading Slack file"),
                }
            }
        }
        // Download and base64-encode images for Claude vision.
        let is_image = file
            .mimetype
            .as_deref()
            .map(|m| {
                matches!(
                    m,
                    "image/jpeg" | "image/png" | "image/gif" | "image/webp"
                )
            })
            .unwrap_or(false);
        let within_image_limit = file
            .size
            .map(|s| s <= MAX_IMAGE_BYTES)
            .unwrap_or(true);

        if is_image && within_image_limit && file.base64_content.is_none() {
            let url = file
                .url_private_download
                .as_deref()
                .or(file.url_private.as_deref());
            if let Some(url) = url {
                match http_client.get(url).bearer_auth(bot_token).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        match resp.bytes().await {
                            Ok(bytes) => {
                                use base64::Engine as _;
                                file.base64_content = Some(
                                    base64::engine::general_purpose::STANDARD.encode(&bytes),
                                );
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to read image file bytes");
                            }
                        }
                    }
                    Ok(resp) => {
                        tracing::warn!(
                            status = %resp.status(),
                            "Non-success status downloading image file"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "HTTP error downloading image file");
                    }
                }
            }
        }
        out.push(file);
    }
    out
}

/// Map slack-morphism `SlackFile` objects to our `OurSlackFile` type.
fn extract_files(content: Option<&SlackMessageContent>) -> Vec<OurSlackFile> {
    let Some(c) = content else { return vec![] };
    let Some(files) = &c.files else { return vec![] };
    files
        .iter()
        .map(|f| OurSlackFile {
            id: Some(f.id.0.clone()),
            name: f.name.clone(),
            mimetype: f.mimetype.as_ref().map(|m| m.0.clone()),
            url_private: f.url_private.as_ref().map(|u| u.to_string()),
            url_private_download: f.url_private_download.as_ref().map(|u| u.to_string()),
            size: None,
            content: None,
            base64_content: None,
        })
        .collect()
}

/// Map slack-morphism message attachments to our `SlackAttachment` type.
fn extract_attachments(content: Option<&SlackMessageContent>) -> Vec<SlackAttachment> {
    let Some(c) = content else { return vec![] };
    let Some(attachments) = &c.attachments else {
        return vec![];
    };
    attachments
        .iter()
        .map(|a| SlackAttachment {
            fallback: a.fallback.clone(),
            text: a.text.clone(),
            pretext: None,
            author_name: None,
            from_url: None,
            image_url: None,
            thumb_url: None,
            channel_id: None,
            channel_name: None,
            ts: None,
            files: vec![],
        })
        .collect()
}

/// Compute the OpenClaw-style session key for routing conversation context.
///
/// - DM:      `slack:dm:<userId>`
/// - Channel: `slack:channel:<channelId>` (or `…:thread:<threadTs>`)
/// - Group:   `slack:group:<channelId>`   (or `…:thread:<threadTs>`)
fn compute_session_key(
    session_type: &SessionType,
    channel_id: &str,
    user_id: &str,
    thread_ts: Option<&str>,
) -> Option<String> {
    let key = match session_type {
        SessionType::Direct => format!("slack:dm:{}", user_id),
        SessionType::Channel => match thread_ts {
            Some(tts) => format!("slack:channel:{}:thread:{}", channel_id, tts),
            None => format!("slack:channel:{}", channel_id),
        },
        SessionType::Group => match thread_ts {
            Some(tts) => format!("slack:group:{}:thread:{}", channel_id, tts),
            None => format!("slack:group:{}", channel_id),
        },
    };
    Some(key)
}

/// Remove any `<@BOT_USER_ID>` mention from text and trim whitespace.
fn strip_bot_mention(text: &str, bot_user_id: Option<&str>) -> String {
    match bot_user_id {
        Some(uid) => text.replace(&format!("<@{}>", uid), "").trim().to_string(),
        None => text.trim().to_string(),
    }
}

pub fn error_handler(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    states: SlackClientEventsUserState,
) -> HttpStatusCode {
    // Attempt to intercept pin_added / pin_removed events that fail to
    // deserialise because slack-morphism v2.x has no typed variants for them
    // in SlackEventCallbackBody.  When the outer Socket Mode message cannot be
    // parsed the library stores the raw JSON body inside a ProtocolError; we
    // fish it out here and handle it before falling through to the generic
    // error log.
    if let Some(slack_err) = err.downcast_ref::<SlackClientError>() {
        if let SlackClientError::ProtocolError(proto_err) = slack_err {
            if let Some(raw_body) = &proto_err.json_body {
                if let Some(pin_event) = try_parse_pin_event(raw_body) {
                    // Obtain the NATS context without blocking the thread: the
                    // futures-locks RwLock exposes a non-blocking try_read().
                    if let Ok(guard) = states.try_read() {
                        if let Some(state) = guard.get_user_state::<BotState>() {
                            let nats = state.nats.clone();
                            tokio::spawn(async move {
                                if let Err(e) = publish_pin(&nats, &pin_event).await {
                                    tracing::error!(
                                        error = %e,
                                        kind  = ?pin_event.kind,
                                        "Failed to publish pin event to NATS"
                                    );
                                }
                            });
                            return HttpStatusCode::OK;
                        }
                    }
                    // State not yet available; fall through to the generic log.
                }
            }
        }
    }

    tracing::error!(error = %err, "Slack socket mode error");
    HttpStatusCode::OK
}

/// Try to parse a raw Socket Mode JSON body as a `pin_added` or `pin_removed`
/// event.
///
/// Slack delivers pin events wrapped in an `events_api` envelope:
/// ```json
/// {
///   "type": "events_api",
///   "payload": {
///     "type": "event_callback",
///     "event": {
///       "type": "pin_added",   // or "pin_removed"
///       "user": "U…",
///       "channel_id": "C…",
///       "item": { "type": "message", "message": { "ts": "…" } },
///       "event_ts": "…"
///     }
///   }
/// }
/// ```
///
/// Returns `None` if the body is not a pin event or cannot be parsed.
fn try_parse_pin_event(raw_body: &str) -> Option<SlackPinEvent> {
    let v: serde_json::Value = serde_json::from_str(raw_body).ok()?;

    // Accept both the Socket Mode envelope ("events_api" wrapping a
    // "event_callback" payload) and bare event_callback payloads.
    let event_obj = if v["type"] == "events_api" {
        &v["payload"]["event"]
    } else if v["type"] == "event_callback" {
        &v["event"]
    } else {
        return None;
    };

    let event_type = event_obj["type"].as_str()?;
    let kind = match event_type {
        "pin_added" => PinEventKind::Added,
        "pin_removed" => PinEventKind::Removed,
        _ => return None,
    };

    let user = event_obj["user"].as_str()?.to_string();

    // Slack may use either "channel_id" or "channel" for pin events.
    let channel = event_obj["channel_id"]
        .as_str()
        .or_else(|| event_obj["channel"].as_str())?
        .to_string();

    let event_ts = event_obj["event_ts"].as_str()?.to_string();

    // The timestamp of the pinned message lives at item.message.ts.
    let item_ts = event_obj["item"]["message"]["ts"]
        .as_str()
        .map(str::to_string);

    let item_type = event_obj["item"]["type"].as_str().map(str::to_string);

    Some(SlackPinEvent {
        kind,
        channel,
        user,
        item_ts,
        item_type,
        event_ts,
    })
}

/// Look up a user's display name via the Slack `users.info` API.
///
/// Results are cached in the module-level `USER_DISPLAY_NAME_CACHE` to avoid
/// repeated API calls. Returns `None` if the API call fails or the user has
/// no name set.
async fn lookup_display_name(
    user_id: &str,
    bot_token: &str,
    http_client: &HttpClient,
) -> Option<String> {
    // Fast path: cache hit.
    {
        let cache = USER_DISPLAY_NAME_CACHE.lock().expect("user cache lock poisoned");
        if let Some(name) = cache.get(user_id) {
            return Some(name.clone());
        }
    }

    // Slow path: call users.info.
    let url = format!("https://slack.com/api/users.info?user={user_id}");
    let resp: serde_json::Value = match http_client
        .get(&url)
        .bearer_auth(bot_token)
        .send()
        .await
    {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse users.info response");
                return None;
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, user = %user_id, "HTTP error calling users.info");
            return None;
        }
    };

    if !resp["ok"].as_bool().unwrap_or(false) {
        tracing::warn!(
            api_error = resp["error"].as_str().unwrap_or("unknown"),
            user = %user_id,
            "users.info failed"
        );
        return None;
    }

    // Prefer profile.display_name, then real_name, then name.
    let name = resp["user"]["profile"]["display_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| resp["user"]["real_name"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| resp["user"]["name"].as_str().filter(|s| !s.is_empty()))
        .map(str::to_string)?;

    USER_DISPLAY_NAME_CACHE
        .lock()
        .expect("user cache lock poisoned")
        .insert(user_id.to_string(), name.clone());
    Some(name)
}

/// Fetch the workspace custom emoji list from `emoji.list`, caching the result.
/// Returns an empty map on failure or if the API returns no emoji.
async fn fetch_custom_emoji(
    bot_token: &str,
    http_client: &HttpClient,
) -> HashMap<String, String> {
    {
        let cache = EMOJI_CACHE.lock().expect("emoji cache lock poisoned");
        if let Some(ref map) = *cache {
            return map.clone();
        }
    }

    match http_client
        .get("https://slack.com/api/emoji.list")
        .bearer_auth(bot_token)
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(body) if body["ok"].as_bool().unwrap_or(false) => {
                let map: HashMap<String, String> = body["emoji"]
                    .as_object()
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                *EMOJI_CACHE.lock().expect("emoji cache lock poisoned") = Some(map.clone());
                map
            }
            Ok(body) => {
                tracing::warn!(
                    api_error = body["error"].as_str().unwrap_or("unknown"),
                    "emoji.list failed"
                );
                HashMap::new()
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse emoji.list response");
                HashMap::new()
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "HTTP error calling emoji.list");
            HashMap::new()
        }
    }
}

/// Determine whether mention gating is active for a given channel/session.
///
/// Priority order:
/// 1. DMs and thread replies → never gated.
/// 2. `no_mention_channels` → gating OFF.
/// 3. `mention_gating_channels` → gating ON.
/// 4. Global `mention_gating` flag.
fn resolve_mention_gating(
    session_type: &SessionType,
    channel_id: &str,
    thread_ts: Option<&str>,
    mention_gating: bool,
    mention_gating_channels: &HashSet<String>,
    no_mention_channels: &HashSet<String>,
    text: &str,
    mention_patterns: &[String],
) -> bool {
    if matches!(session_type, SessionType::Direct) || thread_ts.is_some() {
        return false;
    }
    // Pattern match bypasses gating regardless of channel settings.
    if !mention_patterns.is_empty() {
        let text_lower = text.to_lowercase();
        if mention_patterns.iter().any(|p| text_lower.contains(&p.to_lowercase())) {
            return false; // gating OFF = process the message
        }
    }
    if no_mention_channels.contains(channel_id) {
        return false;
    }
    if mention_gating_channels.contains(channel_id) {
        return true;
    }
    mention_gating
}

/// Replace `:name:` emoji shortcodes that appear in the workspace's custom
/// emoji list with `:name: [custom emoji]` so Claude can identify them.
fn annotate_custom_emoji(text: &str, custom_emoji: &HashMap<String, String>) -> String {
    if custom_emoji.is_empty() || !text.contains(':') {
        return text.to_string();
    }
    let mut result = text.to_string();
    for name in custom_emoji.keys() {
        let pattern = format!(":{name}:");
        if result.contains(&pattern) {
            result = result.replace(&pattern, &format!(":{name}: [custom emoji]"));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── strip_bot_mention ─────────────────────────────────────────────────────

    #[test]
    fn strip_removes_known_mention() {
        assert_eq!(
            strip_bot_mention("<@UBOT123> hello", Some("UBOT123")),
            "hello"
        );
    }

    #[test]
    fn strip_removes_mention_mid_text() {
        assert_eq!(
            strip_bot_mention("hey <@UBOT123> what's up", Some("UBOT123")),
            "hey  what's up"
        );
    }

    #[test]
    fn strip_no_bot_id_only_trims() {
        assert_eq!(strip_bot_mention("  hello  ", None), "hello");
    }

    #[test]
    fn strip_unknown_bot_id_leaves_text() {
        assert_eq!(
            strip_bot_mention("<@UOTHER> hello", Some("UBOT123")),
            "<@UOTHER> hello"
        );
    }

    #[test]
    fn strip_empty_text() {
        assert_eq!(strip_bot_mention("", Some("UBOT123")), "");
    }

    // ── compute_session_key ───────────────────────────────────────────────────

    #[test]
    fn session_key_dm() {
        assert_eq!(
            compute_session_key(&SessionType::Direct, "C1", "U1", None),
            Some("slack:dm:U1".to_string())
        );
    }

    #[test]
    fn session_key_channel_no_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Channel, "C1", "U1", None),
            Some("slack:channel:C1".to_string())
        );
    }

    #[test]
    fn session_key_channel_with_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Channel, "C1", "U1", Some("1234.5")),
            Some("slack:channel:C1:thread:1234.5".to_string())
        );
    }

    #[test]
    fn session_key_group_with_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Group, "G1", "U1", Some("9.0")),
            Some("slack:group:G1:thread:9.0".to_string())
        );
    }

    // ── resolve_mention_gating ────────────────────────────────────────────────

    fn make_sets(channels: &[&str]) -> HashSet<String> {
        channels.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn gating_dm_always_off() {
        assert!(!resolve_mention_gating(
            &SessionType::Direct, "C1", None, true, &make_sets(&["C1"]), &HashSet::new(), "", &[]
        ));
    }

    #[test]
    fn gating_thread_reply_always_off() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel, "C1", Some("123.0"), true, &HashSet::new(), &HashSet::new(), "", &[]
        ));
    }

    #[test]
    fn gating_no_mention_channel_overrides_global_on() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel, "C1", None, true, &HashSet::new(), &make_sets(&["C1"]), "", &[]
        ));
    }

    #[test]
    fn gating_mention_gating_channel_overrides_global_off() {
        assert!(resolve_mention_gating(
            &SessionType::Channel, "C1", None, false, &make_sets(&["C1"]), &HashSet::new(), "", &[]
        ));
    }

    #[test]
    fn gating_falls_back_to_global_true() {
        assert!(resolve_mention_gating(
            &SessionType::Channel, "C2", None, true, &make_sets(&["C1"]), &make_sets(&["C3"]), "", &[]
        ));
    }

    #[test]
    fn gating_falls_back_to_global_false() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel, "C2", None, false, &make_sets(&["C1"]), &make_sets(&["C3"]), "", &[]
        ));
    }

    #[test]
    fn gating_no_mention_takes_precedence_over_gating_channel() {
        // If a channel is in both sets, no_mention_channels wins (checked first).
        assert!(!resolve_mention_gating(
            &SessionType::Channel, "C1", None, true, &make_sets(&["C1"]), &make_sets(&["C1"]), "", &[]
        ));
    }

    // ── annotate_custom_emoji ─────────────────────────────────────────────────

    #[test]
    fn annotate_custom_emoji_replaces_known() {
        let mut map = HashMap::new();
        map.insert("company_logo".to_string(), "https://emoji.example.com/logo.png".to_string());
        let result = annotate_custom_emoji("Great :company_logo: work!", &map);
        assert!(result.contains(":company_logo: [custom emoji]"));
    }

    #[test]
    fn annotate_custom_emoji_leaves_standard_alone() {
        let mut map = HashMap::new();
        map.insert("custom_one".to_string(), "url".to_string());
        let result = annotate_custom_emoji("Hello :thumbsup: world", &map);
        assert_eq!(result, "Hello :thumbsup: world");
    }

    #[test]
    fn annotate_custom_emoji_empty_map_returns_unchanged() {
        let result = annotate_custom_emoji("Hello :custom:", &HashMap::new());
        assert_eq!(result, "Hello :custom:");
    }

    #[test]
    fn gating_pattern_match_bypasses_gating() {
        // Even with gating=true, a message matching a pattern is allowed through.
        let patterns = vec!["hey bot".to_string()];
        let result = resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            true,  // gating ON
            &HashSet::new(),
            &HashSet::new(),
            "hey bot can you help?",
            &patterns,
        );
        assert!(!result, "gating should be OFF when pattern matches");
    }

    #[test]
    fn gating_pattern_case_insensitive() {
        let patterns = vec!["hey bot".to_string()];
        let result = resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            true,
            &HashSet::new(),
            &HashSet::new(),
            "HEY BOT please help",
            &patterns,
        );
        assert!(!result);
    }

    #[test]
    fn gating_no_patterns_does_not_affect_behavior() {
        // Empty patterns — gating still applies normally
        let result = resolve_mention_gating(
            &SessionType::Channel,
            "C1",
            None,
            true,
            &HashSet::new(),
            &HashSet::new(),
            "random message",
            &[],
        );
        assert!(result, "gating should still be ON with no patterns");
    }
}

/// Send unfurl payloads to Slack for a set of URLs found in a `link_shared` event.
async fn unfurl_links(
    bot_token: &str,
    http_client: &HttpClient,
    channel: &str,
    message_ts: &str,
    urls: &[String],
) {
    let mut unfurls = serde_json::Map::new();
    for url in urls {
        let (title, description) = fetch_url_metadata(http_client, url).await;
        unfurls.insert(
            url.clone(),
            serde_json::json!({
                "title": title,
                "text": description,
            }),
        );
    }
    if unfurls.is_empty() {
        return;
    }

    let body = serde_json::json!({
        "channel": channel,
        "ts": message_ts,
        "unfurls": unfurls,
    });

    match http_client
        .post("https://slack.com/api/chat.unfurl")
        .header("Authorization", format!("Bearer {}", bot_token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(v) if v["ok"].as_bool() == Some(true) => {
                tracing::debug!(channel, message_ts, "chat.unfurl succeeded");
            }
            Ok(v) => {
                tracing::warn!(
                    error = v["error"].as_str().unwrap_or("unknown"),
                    "chat.unfurl returned ok=false"
                );
            }
            Err(e) => tracing::warn!(error = %e, "Failed to parse chat.unfurl response"),
        },
        Err(e) => tracing::warn!(error = %e, "HTTP error calling chat.unfurl"),
    }
}

/// Fetch a URL and extract its `<title>` and meta description.
async fn fetch_url_metadata(http_client: &HttpClient, url: &str) -> (String, String) {
    let resp = match http_client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (compatible; TrogonBot/1.0)")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return (url.to_string(), String::new()),
    };
    let html = match resp.text().await {
        Ok(h) => h,
        Err(_) => return (url.to_string(), String::new()),
    };

    let title = extract_between(&html, "<title>", "</title>")
        .unwrap_or_else(|| url.to_string());

    // Try <meta name="description" content="...">
    // We search for the pattern in lowercased HTML, then extract from the original.
    let description = extract_meta_description(&html).unwrap_or_default();

    (title.trim().to_string(), description.trim().to_string())
}

/// Extract the substring between `open` and `close` in `html` (case-insensitive open tag match).
fn extract_between(html: &str, open: &str, close: &str) -> Option<String> {
    let html_lower = html.to_lowercase();
    let open_lower = open.to_lowercase();
    let close_lower = close.to_lowercase();
    let start = html_lower.find(&open_lower)? + open.len();
    let end = html_lower[start..].find(&close_lower)?;
    Some(html[start..start + end].to_string())
}

/// Extract the content of a `<meta name="description" content="...">` tag.
fn extract_meta_description(html: &str) -> Option<String> {
    let html_lower = html.to_lowercase();
    // Find a <meta ... containing name="description" or name='description'
    let mut search_from = 0;
    while let Some(tag_start) = html_lower[search_from..].find("<meta") {
        let abs_tag_start = search_from + tag_start;
        // Find the end of this tag
        let tag_end = html_lower[abs_tag_start..]
            .find('>')
            .map(|e| abs_tag_start + e + 1)
            .unwrap_or(html_lower.len());
        let tag_lower = &html_lower[abs_tag_start..tag_end];
        let tag_orig = &html[abs_tag_start..tag_end];

        if tag_lower.contains(r#"name="description""#) || tag_lower.contains("name='description'") {
            // Extract content="..." or content='...'
            if let Some(val) = extract_attr_value(tag_orig, "content") {
                return Some(val);
            }
        }
        search_from = tag_end;
    }
    None
}

/// Extract an attribute value from a tag string (handles both quote styles).
fn extract_attr_value(tag: &str, attr: &str) -> Option<String> {
    let tag_lower = tag.to_lowercase();
    // Try double-quoted value: attr="..."
    let needle_dq = format!("{}=\"", attr);
    if let Some(pos) = tag_lower.find(&needle_dq) {
        let start = pos + needle_dq.len();
        let end = tag[start..].find('"')?;
        return Some(tag[start..start + end].to_string());
    }
    // Try single-quoted value: attr='...'
    // Build the needle as a String to avoid raw character literal parsing issues.
    let sq: char = '\'';
    let needle_sq = format!("{}={}", attr, sq);
    if let Some(pos) = tag_lower.find(&needle_sq) {
        let start = pos + needle_sq.len();
        let end = tag[start..].find(sq)?;
        return Some(tag[start..start + end].to_string());
    }
    None
}
