use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use slack_nats::publisher::{
    publish_app_home, publish_block_action, publish_channel, publish_inbound, publish_link_shared,
    publish_member, publish_message_changed, publish_message_deleted, publish_pin,
    publish_reaction, publish_slash_command, publish_thread_broadcast, publish_view_closed,
    publish_view_submission,
};
use slack_types::enrich::{
    annotate_custom_emoji, compute_session_key, resolve_mention_gating, strip_bot_mention,
};
use slack_types::events::{
    ChannelEventKind, PinEventKind, SessionType, SlackAppHomeOpenedEvent, SlackAttachment,
    SlackBlockActionEvent, SlackChannelEvent, SlackFile, SlackInboundMessage, SlackLinkSharedEvent,
    SlackMemberEvent, SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackPinEvent,
    SlackReactionEvent, SlackSlashCommandEvent, SlackThreadBroadcastEvent, SlackViewClosedEvent,
    SlackViewSubmissionEvent,
};
use trogon_nats::PublishClient;

use crate::config::EnricherConfig;

const MAX_IMAGE_BYTES: u64 = 5_000_000;

static USER_DISPLAY_NAME_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static EMOJI_CACHE: LazyLock<Mutex<Option<HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(None));

pub struct EnrichContext {
    pub config: EnricherConfig,
    pub http_client: reqwest::Client,
}

// ── Events API dispatch ─────────────────────────────────────────────────────

pub async fn handle_raw_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    payload: &[u8],
    nats_headers: &async_nats::HeaderMap,
) where
    C::PublishError: 'static,
{
    let event_type = nats_headers
        .get("X-Slack-Event-Type")
        .map(|v| v.as_str());

    let body: serde_json::Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse raw event payload");
            return;
        }
    };

    // trogon-source-slack publishes the full event_callback body.
    // The inner event is at body["event"] for Events API payloads.
    let event = if body.get("event").is_some() {
        &body["event"]
    } else {
        &body
    };

    let et = event_type.unwrap_or_else(|| event["type"].as_str().unwrap_or(""));

    match et {
        "message" => handle_message_event(js, ctx, event).await,
        "app_mention" => handle_app_mention_event(js, ctx, event).await,
        "reaction_added" => handle_reaction_event(js, ctx, event, true).await,
        "reaction_removed" => handle_reaction_event(js, ctx, event, false).await,
        "member_joined_channel" => handle_member_event(js, ctx, event, true).await,
        "member_left_channel" => handle_member_event(js, ctx, event, false).await,
        "channel_created" => handle_channel_event(js, ctx, event, ChannelEventKind::Created).await,
        "channel_deleted" => handle_channel_event(js, ctx, event, ChannelEventKind::Deleted).await,
        "channel_archive" => handle_channel_event(js, ctx, event, ChannelEventKind::Archived).await,
        "channel_unarchive" => {
            handle_channel_event(js, ctx, event, ChannelEventKind::Unarchived).await;
        }
        "channel_rename" => handle_channel_event(js, ctx, event, ChannelEventKind::Renamed).await,
        "app_home_opened" => handle_app_home_event(js, ctx, event).await,
        "link_shared" => handle_link_shared_event(js, ctx, event).await,
        "pin_added" => handle_pin_event(js, ctx, event, PinEventKind::Added).await,
        "pin_removed" => handle_pin_event(js, ctx, event, PinEventKind::Removed).await,
        other => {
            tracing::debug!(event_type = other, "Ignoring unhandled Slack event type");
        }
    }
}

// ── Interaction dispatch ────────────────────────────────────────────────────

pub async fn handle_raw_interaction<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    payload: &[u8],
) where
    C::PublishError: 'static,
{
    let body: serde_json::Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse raw interaction payload");
            return;
        }
    };

    let interaction_type = body["type"].as_str().unwrap_or("");
    match interaction_type {
        "block_actions" => {
            let channel_id = body["channel"]["id"].as_str().map(String::from);
            let user_id = body["user"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let message_ts = body["message"]["ts"].as_str().map(String::from);
            let trigger_id = body["trigger_id"].as_str().map(String::from);

            if let Some(actions) = body["actions"].as_array() {
                for action in actions {
                    let block_action_ev = SlackBlockActionEvent {
                        action_id: action["action_id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        block_id: action["block_id"].as_str().map(String::from),
                        user_id: user_id.clone(),
                        channel_id: channel_id.clone(),
                        message_ts: message_ts.clone(),
                        value: action["value"].as_str().map(String::from),
                        trigger_id: trigger_id.clone(),
                    };

                    if let Err(e) = publish_block_action(
                        js,
                        ctx.config.account_id.as_deref(),
                        &block_action_ev,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Failed to publish block_action");
                    }
                }
            }
        }
        "view_submission" => {
            let view_ev = SlackViewSubmissionEvent {
                user_id: body["user"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                trigger_id: body["trigger_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                view_id: body["view"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                callback_id: body["view"]["callback_id"].as_str().map(String::from),
                values: body["view"]["state"]["values"].clone(),
            };

            if let Err(e) =
                publish_view_submission(js, ctx.config.account_id.as_deref(), &view_ev).await
            {
                tracing::error!(error = %e, "Failed to publish view_submission");
            }
        }
        "view_closed" => {
            let view_ev = SlackViewClosedEvent {
                user_id: body["user"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                trigger_id: body["trigger_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                view_id: body["view"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                callback_id: body["view"]["callback_id"].as_str().map(String::from),
                values: serde_json::Value::Null,
            };

            if let Err(e) =
                publish_view_closed(js, ctx.config.account_id.as_deref(), &view_ev).await
            {
                tracing::error!(error = %e, "Failed to publish view_closed");
            }
        }
        _ => {
            tracing::debug!(
                interaction_type,
                "Ignoring unhandled interaction type"
            );
        }
    }
}

// ── Slash command dispatch ──────────────────────────────────────────────────

pub async fn handle_raw_command<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    payload: &[u8],
) where
    C::PublishError: 'static,
{
    let body_str = std::str::from_utf8(payload).unwrap_or("");
    let params: HashMap<String, String> = form_urlencoded::parse(body_str.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let slash_ev = SlackSlashCommandEvent {
        command: params.get("command").cloned().unwrap_or_default(),
        text: params.get("text").cloned(),
        user_id: params.get("user_id").cloned().unwrap_or_default(),
        channel_id: params.get("channel_id").cloned().unwrap_or_default(),
        team_id: params.get("team_id").cloned(),
        response_url: params.get("response_url").cloned().unwrap_or_default(),
        trigger_id: params.get("trigger_id").cloned(),
    };

    if let Err(e) =
        publish_slash_command(js, ctx.config.account_id.as_deref(), &slash_ev).await
    {
        tracing::error!(error = %e, "Failed to publish slash command");
    }
}

// ── Event handlers ──────────────────────────────────────────────────────────

async fn handle_message_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
) where
    C::PublishError: 'static,
{
    if event["bot_id"].is_string() && !ctx.config.allow_bots {
        return;
    }

    let subtype = event["subtype"].as_str();

    match subtype {
        Some("message_changed") => {
            let ev = SlackMessageChangedEvent {
                channel: event["channel"].as_str().unwrap_or_default().to_string(),
                ts: event["message"]["ts"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                event_ts: event["event_ts"].as_str().map(String::from),
                previous_text: event["previous_message"]["text"].as_str().map(String::from),
                new_text: event["message"]["text"].as_str().map(String::from),
                thread_ts: event["message"]["thread_ts"].as_str().map(String::from),
                user: event["message"]["user"].as_str().map(String::from),
            };
            if let Err(e) =
                publish_message_changed(js, ctx.config.account_id.as_deref(), &ev).await
            {
                tracing::error!(error = %e, "Failed to publish message_changed");
            }
        }
        Some("message_deleted") => {
            let ev = SlackMessageDeletedEvent {
                channel: event["channel"].as_str().unwrap_or_default().to_string(),
                deleted_ts: event["deleted_ts"]
                    .as_str()
                    .unwrap_or_else(|| event["ts"].as_str().unwrap_or_default())
                    .to_string(),
                event_ts: event["event_ts"].as_str().map(String::from),
                thread_ts: event["thread_ts"].as_str().map(String::from),
            };
            if let Err(e) =
                publish_message_deleted(js, ctx.config.account_id.as_deref(), &ev).await
            {
                tracing::error!(error = %e, "Failed to publish message_deleted");
            }
        }
        Some("thread_broadcast") => {
            let ts = event["ts"].as_str().unwrap_or_default().to_string();
            let thread_ts = event["thread_ts"]
                .as_str()
                .unwrap_or(&ts)
                .to_string();
            let ev = SlackThreadBroadcastEvent {
                channel: event["channel"].as_str().unwrap_or_default().to_string(),
                user: event["user"].as_str().unwrap_or_default().to_string(),
                text: event["text"].as_str().unwrap_or_default().to_string(),
                ts,
                thread_ts,
                event_ts: None,
            };
            if let Err(e) =
                publish_thread_broadcast(js, ctx.config.account_id.as_deref(), &ev).await
            {
                tracing::error!(error = %e, "Failed to publish thread_broadcast");
            }
        }
        Some(_) => {}
        None => {
            let user = event["user"].as_str().unwrap_or_default().to_string();

            if let Some(ref bot_uid) = ctx.config.bot_user_id {
                if &user == bot_uid {
                    return;
                }
            }

            let channel = event["channel"].as_str().unwrap_or_default().to_string();
            let raw_text = event["text"].as_str().unwrap_or_default().to_string();
            let text = strip_bot_mention(&raw_text, ctx.config.bot_user_id.as_deref());

            if text.is_empty() {
                return;
            }

            let custom_emoji = fetch_custom_emoji(&ctx.config.bot_token, &ctx.http_client).await;
            let text = annotate_custom_emoji(&text, &custom_emoji);

            let ts = event["ts"].as_str().unwrap_or_default().to_string();
            let thread_ts = event["thread_ts"].as_str().map(String::from);

            let session_type = match event["channel_type"].as_str() {
                Some("im") => SessionType::Direct,
                Some("mpim") => SessionType::Group,
                _ => SessionType::Channel,
            };

            if resolve_mention_gating(
                &session_type,
                &channel,
                thread_ts.as_deref(),
                ctx.config.mention_gating,
                &ctx.config.mention_gating_channels,
                &ctx.config.no_mention_channels,
                &text,
                &ctx.config.mention_patterns,
            ) {
                if let Some(ref bot_uid) = ctx.config.bot_user_id {
                    if !raw_text.contains(&format!("<@{}>", bot_uid)) {
                        return;
                    }
                }
            }

            let session_key =
                compute_session_key(&session_type, &channel, &user, thread_ts.as_deref());

            let files = extract_and_fetch_files(event, ctx).await;
            let attachments = extract_attachments(event);
            let display_name =
                lookup_display_name(&user, &ctx.config.bot_token, &ctx.http_client).await;

            let inbound = SlackInboundMessage {
                channel,
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

            if let Err(e) =
                publish_inbound(js, ctx.config.account_id.as_deref(), &inbound).await
            {
                tracing::error!(error = %e, "Failed to publish inbound message");
            }
        }
    }
}

async fn handle_app_mention_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
) where
    C::PublishError: 'static,
{
    let user = event["user"].as_str().unwrap_or_default().to_string();
    let channel = event["channel"].as_str().unwrap_or_default().to_string();
    let raw_text = event["text"].as_str().unwrap_or_default().to_string();
    let text = strip_bot_mention(&raw_text, ctx.config.bot_user_id.as_deref());

    if text.is_empty() {
        return;
    }

    let custom_emoji = fetch_custom_emoji(&ctx.config.bot_token, &ctx.http_client).await;
    let text = annotate_custom_emoji(&text, &custom_emoji);

    let ts = event["ts"].as_str().unwrap_or_default().to_string();
    let thread_ts = event["thread_ts"].as_str().map(String::from);
    let session_key =
        compute_session_key(&SessionType::Channel, &channel, &user, thread_ts.as_deref());

    let files = extract_and_fetch_files(event, ctx).await;
    let display_name =
        lookup_display_name(&user, &ctx.config.bot_token, &ctx.http_client).await;

    let inbound = SlackInboundMessage {
        channel,
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

    if let Err(e) = publish_inbound(js, ctx.config.account_id.as_deref(), &inbound).await {
        tracing::error!(error = %e, "Failed to publish app_mention");
    }
}

async fn handle_reaction_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
    added: bool,
) where
    C::PublishError: 'static,
{
    let item = &event["item"];
    let channel = if item["type"].as_str() == Some("message") {
        item["channel"].as_str().map(String::from)
    } else {
        None
    };
    let item_ts = if item["type"].as_str() == Some("message") {
        item["ts"].as_str().map(String::from)
    } else {
        None
    };

    let ev = SlackReactionEvent {
        reaction: event["reaction"].as_str().unwrap_or_default().to_string(),
        user: event["user"].as_str().unwrap_or_default().to_string(),
        channel,
        item_ts,
        item_user: event["item_user"].as_str().map(String::from),
        event_ts: event["event_ts"].as_str().unwrap_or_default().to_string(),
        added,
    };

    if let Err(e) = publish_reaction(js, ctx.config.account_id.as_deref(), &ev).await {
        tracing::error!(error = %e, added, "Failed to publish reaction event");
    }
}

async fn handle_member_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
    joined: bool,
) where
    C::PublishError: 'static,
{
    let ev = SlackMemberEvent {
        user: event["user"].as_str().unwrap_or_default().to_string(),
        channel: event["channel"].as_str().unwrap_or_default().to_string(),
        channel_type: event["channel_type"].as_str().map(String::from),
        team: event["team"].as_str().map(String::from),
        inviter: event["inviter"].as_str().map(String::from),
        joined,
    };

    if let Err(e) = publish_member(js, ctx.config.account_id.as_deref(), &ev).await {
        tracing::error!(error = %e, joined, "Failed to publish member event");
    }
}

async fn handle_channel_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
    kind: ChannelEventKind,
) where
    C::PublishError: 'static,
{
    let (channel_id, channel_name, user) = match kind {
        ChannelEventKind::Created | ChannelEventKind::Renamed => {
            let ch = &event["channel"];
            (
                ch["id"].as_str().unwrap_or_default().to_string(),
                ch["name"].as_str().map(String::from),
                ch["creator"].as_str().map(String::from),
            )
        }
        _ => (
            event["channel"].as_str().unwrap_or_default().to_string(),
            None,
            event["user"].as_str().map(String::from),
        ),
    };

    let ev = SlackChannelEvent {
        kind,
        channel_id,
        channel_name,
        user,
    };

    if let Err(e) = publish_channel(js, ctx.config.account_id.as_deref(), &ev).await {
        tracing::error!(error = %e, "Failed to publish channel event");
    }
}

async fn handle_app_home_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
) where
    C::PublishError: 'static,
{
    let ev = SlackAppHomeOpenedEvent {
        user: event["user"].as_str().unwrap_or_default().to_string(),
        tab: event["tab"].as_str().unwrap_or_default().to_string(),
        view_id: event["view"]["id"].as_str().map(String::from),
    };

    if let Err(e) = publish_app_home(js, ctx.config.account_id.as_deref(), &ev).await {
        tracing::error!(error = %e, "Failed to publish app_home_opened");
    }
}

async fn handle_link_shared_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
) where
    C::PublishError: 'static,
{
    let links = event["links"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["url"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let ev = SlackLinkSharedEvent {
        channel: event["channel"].as_str().unwrap_or_default().to_string(),
        message_ts: event["message_ts"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        event_ts: event["event_ts"].as_str().map(String::from),
        links,
    };

    if let Err(e) = publish_link_shared(js, ctx.config.account_id.as_deref(), &ev).await {
        tracing::error!(error = %e, "Failed to publish link_shared");
    }
}

async fn handle_pin_event<C: PublishClient>(
    js: &C,
    ctx: &EnrichContext,
    event: &serde_json::Value,
    kind: PinEventKind,
) where
    C::PublishError: 'static,
{
    let channel = event["channel_id"]
        .as_str()
        .or_else(|| event["channel"].as_str())
        .unwrap_or_default()
        .to_string();

    let ev = SlackPinEvent {
        kind,
        channel,
        user: event["user"].as_str().unwrap_or_default().to_string(),
        item_ts: event["item"]["message"]["ts"]
            .as_str()
            .map(String::from),
        item_type: event["item"]["type"].as_str().map(String::from),
        event_ts: event["event_ts"].as_str().unwrap_or_default().to_string(),
    };

    if let Err(e) = publish_pin(js, ctx.config.account_id.as_deref(), &ev).await {
        tracing::error!(error = %e, "Failed to publish pin event");
    }
}

// ── I/O helpers ─────────────────────────────────────────────────────────────

async fn extract_and_fetch_files(
    event: &serde_json::Value,
    ctx: &EnrichContext,
) -> Vec<SlackFile> {
    let files_json = match event["files"].as_array() {
        Some(arr) => arr.clone(),
        None => return vec![],
    };

    let mut out = Vec::new();
    for f in &files_json {
        let mut file = SlackFile {
            id: f["id"].as_str().map(String::from),
            name: f["name"].as_str().map(String::from),
            mimetype: f["mimetype"].as_str().map(String::from),
            url_private: f["url_private"].as_str().map(String::from),
            url_private_download: f["url_private_download"].as_str().map(String::from),
            size: f["size"].as_u64(),
            content: None,
            base64_content: None,
        };

        let within_limit = file
            .size
            .map(|s| s <= ctx.config.media_max_mb * 1024 * 1024)
            .unwrap_or(true);

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

        if is_text && within_limit {
            if let Some(url) = file
                .url_private_download
                .as_deref()
                .or(file.url_private.as_deref())
            {
                match ctx
                    .http_client
                    .get(url)
                    .bearer_auth(&ctx.config.bot_token)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(text) = resp.text().await {
                            file.content = Some(text);
                        }
                    }
                    Ok(resp) => {
                        tracing::warn!(status = %resp.status(), "Non-success downloading file")
                    }
                    Err(e) => tracing::warn!(error = %e, "HTTP error downloading file"),
                }
            }
        }

        let is_image = file
            .mimetype
            .as_deref()
            .map(|m| matches!(m, "image/jpeg" | "image/png" | "image/gif" | "image/webp"))
            .unwrap_or(false);
        let within_image_limit = file.size.map(|s| s <= MAX_IMAGE_BYTES).unwrap_or(true);

        if is_image && within_image_limit && file.base64_content.is_none() {
            if let Some(url) = file
                .url_private_download
                .as_deref()
                .or(file.url_private.as_deref())
            {
                match ctx
                    .http_client
                    .get(url)
                    .bearer_auth(&ctx.config.bot_token)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(bytes) = resp.bytes().await {
                            use base64::Engine as _;
                            file.base64_content = Some(
                                base64::engine::general_purpose::STANDARD.encode(&bytes),
                            );
                        }
                    }
                    Ok(resp) => {
                        tracing::warn!(status = %resp.status(), "Non-success downloading image")
                    }
                    Err(e) => tracing::warn!(error = %e, "HTTP error downloading image"),
                }
            }
        }

        out.push(file);
    }
    out
}

fn extract_attachments(event: &serde_json::Value) -> Vec<SlackAttachment> {
    event["attachments"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|a| SlackAttachment {
            fallback: a["fallback"].as_str().map(String::from),
            text: a["text"].as_str().map(String::from),
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

async fn lookup_display_name(
    user_id: &str,
    bot_token: &str,
    http_client: &reqwest::Client,
) -> Option<String> {
    {
        let cache = USER_DISPLAY_NAME_CACHE
            .lock()
            .expect("user cache lock poisoned");
        if let Some(name) = cache.get(user_id) {
            return Some(name.clone());
        }
    }

    let url = format!("https://slack.com/api/users.info?user={user_id}");
    let resp: serde_json::Value = http_client
        .get(&url)
        .bearer_auth(bot_token)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    if !resp["ok"].as_bool().unwrap_or(false) {
        return None;
    }

    let name = resp["user"]["profile"]["display_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            resp["user"]["real_name"]
                .as_str()
                .filter(|s| !s.is_empty())
        })
        .or_else(|| resp["user"]["name"].as_str().filter(|s| !s.is_empty()))
        .map(str::to_string)?;

    USER_DISPLAY_NAME_CACHE
        .lock()
        .expect("user cache lock poisoned")
        .insert(user_id.to_string(), name.clone());
    Some(name)
}

async fn fetch_custom_emoji(
    bot_token: &str,
    http_client: &reqwest::Client,
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
            _ => HashMap::new(),
        },
        Err(_) => HashMap::new(),
    }
}

