use async_nats::jetstream::Context as JsContext;
use slack_morphism::prelude::*;
use slack_nats::publisher::{
    publish_block_action, publish_channel, publish_inbound, publish_member,
    publish_message_changed, publish_message_deleted, publish_reaction, publish_slash_command,
    publish_thread_broadcast,
};
use slack_types::events::{
    ChannelEventKind, SessionType, SlackAttachment, SlackBlockActionEvent, SlackChannelEvent,
    SlackFile as OurSlackFile, SlackInboundMessage, SlackMemberEvent, SlackMessageChangedEvent,
    SlackMessageDeletedEvent, SlackReactionEvent, SlackSlashCommandEvent, SlackThreadBroadcastEvent,
};
use std::sync::Arc;

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
}

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
            // Drop messages from any bot (including ourselves).
            if msg.sender.bot_id.is_some() {
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

                    if let Err(e) =
                        publish_message_changed(&*state.nats, &ev).await
                    {
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

                    if let Err(e) =
                        publish_message_deleted(&*state.nats, &ev).await
                    {
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

                    if let Err(e) =
                        publish_thread_broadcast(&*state.nats, &ev).await
                    {
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
                    if let Some(ref bot_uid) = state.bot_user_id {
                        if &user == bot_uid {
                            return Ok(());
                        }
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
                    let text =
                        strip_bot_mention(&raw_text, state.bot_user_id.as_deref());

                    if text.is_empty() {
                        return Ok(());
                    }

                    let ts = msg.origin.ts.0.clone();
                    let thread_ts = msg.origin.thread_ts.as_ref().map(|t| t.0.clone());

                    let session_type = match msg
                        .origin
                        .channel_type
                        .as_ref()
                        .map(|ct| ct.0.as_str())
                    {
                        Some("im") => SessionType::Direct,
                        Some("mpim") => SessionType::Group,
                        _ => SessionType::Channel,
                    };

                    // Mention-gating (OpenClaw: requireMention = true by default).
                    // Rules:
                    //   - DMs always pass through.
                    //   - Thread replies always pass through (bot is already in the thread).
                    //   - Channels/groups: require explicit @mention when gating is on.
                    if state.mention_gating && !matches!(session_type, SessionType::Direct) {
                        if thread_ts.is_none() {
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
                    }

                    // Compute session key for conversation routing.
                    let session_key = compute_session_key(&session_type, &channel_id, &user, thread_ts.as_deref());

                    tracing::debug!(
                        channel = %channel_id,
                        user = %user,
                        session_type = ?session_type,
                        session_key = ?session_key,
                        "Received message, publishing to NATS"
                    );

                    let files = extract_files(msg.content.as_ref());
                    let attachments = extract_attachments(msg.content.as_ref());

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
                    };

                    if let Err(e) = publish_inbound(&*state.nats, &inbound).await {
                        tracing::error!(error = %e, "Failed to publish inbound message to NATS");
                    }
                }
            }
        }

        SlackEventCallbackBody::AppMention(mention) => {
            let channel_id = mention.channel.0.clone();
            let user = mention.user.0.clone();
            let raw_text = mention
                .content
                .text
                .as_deref()
                .unwrap_or("")
                .to_string();
            let text = strip_bot_mention(&raw_text, state.bot_user_id.as_deref());

            if text.is_empty() {
                return Ok(());
            }

            let ts = mention.origin.ts.0.clone();
            let thread_ts = mention.origin.thread_ts.as_ref().map(|t| t.0.clone());

            let session_key = compute_session_key(
                &SessionType::Channel,
                &channel_id,
                &user,
                thread_ts.as_deref(),
            );

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
                files: vec![],
                attachments: vec![],
            };

            if let Err(e) = publish_inbound(&*state.nats, &inbound).await {
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

            if let Err(e) =
                publish_reaction(&*state.nats, &reaction_ev).await
            {
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

            if let Err(e) =
                publish_reaction(&*state.nats, &reaction_ev).await
            {
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

            if let Err(e) =
                publish_member(&*state.nats, &member_ev).await
            {
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

            if let Err(e) =
                publish_member(&*state.nats, &member_ev).await
            {
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

            if let Err(e) =
                publish_channel(&*state.nats, &channel_ev).await
            {
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

            if let Err(e) =
                publish_channel(&*state.nats, &channel_ev).await
            {
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

            if let Err(e) =
                publish_channel(&*state.nats, &channel_ev).await
            {
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

            if let Err(e) =
                publish_channel(&*state.nats, &channel_ev).await
            {
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

            if let Err(e) =
                publish_channel(&*state.nats, &channel_ev).await
            {
                tracing::error!(error = %e, "Failed to publish channel_unarchive to NATS");
            }
        }

        _ => {
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

    if let SlackInteractionEvent::BlockActions(ev) = event {
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

            if let Err(e) = publish_block_action(&*state.nats, &block_action_ev).await {
                tracing::error!(error = %e, "Failed to publish block_action to NATS");
            }
        }
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

    if let Err(e) =
        publish_slash_command(&*state.nats, &slash_ev).await
    {
        tracing::error!(error = %e, "Failed to publish slash command to NATS");
    }

    Ok(SlackCommandEventResponse::new(
        SlackMessageContent::new().with_text("Command received.".into()),
    ))
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
    _states: SlackClientEventsUserState,
) -> HttpStatusCode {
    tracing::error!(error = %err, "Slack socket mode error");
    HttpStatusCode::OK
}
