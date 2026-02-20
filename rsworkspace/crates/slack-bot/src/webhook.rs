//! HTTP Events API server — full complement to (or replacement of) Socket Mode.
//!
//! Handles:
//!   • JSON Events API: event_callback, url_verification
//!   • URL-encoded interactive payloads: block_actions, view_submission, view_closed
//!   • URL-encoded slash command payloads
//!
//! Configure with SLACK_EVENTS_PORT (default 3001) and SLACK_HTTP_PATH (default /slack/events).
//! Set SLACK_SIGNING_SECRET to enable request signature verification (strongly recommended).

use async_nats::jetstream::Context as JsContext;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use slack_nats::publisher::{
    publish_app_home, publish_block_action, publish_channel, publish_inbound, publish_member,
    publish_message_changed, publish_message_deleted, publish_pin, publish_reaction,
    publish_slash_command, publish_thread_broadcast, publish_view_closed, publish_view_submission,
};
use slack_types::events::{
    ChannelEventKind, PinEventKind, SessionType, SlackAppHomeOpenedEvent, SlackAttachment,
    SlackBlockActionEvent, SlackChannelEvent, SlackFile, SlackInboundMessage, SlackMemberEvent,
    SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackPinEvent, SlackReactionEvent,
    SlackSlashCommandEvent, SlackThreadBroadcastEvent, SlackViewClosedEvent,
    SlackViewSubmissionEvent,
};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::net::TcpListener;

type HmacSha256 = Hmac<Sha256>;

/// Maximum image size that will be base64-encoded and forwarded to the agent.
const MAX_IMAGE_BYTES: u64 = 5_000_000;

#[derive(Clone)]
pub struct WebhookState {
    pub signing_secret: Option<String>,
    pub js: Arc<JsContext>,
    pub bot_token: String,
    pub bot_user_id: Option<String>,
    pub allow_bots: bool,
    pub media_max_mb: u64,
    pub mention_gating: bool,
    pub mention_gating_channels: HashSet<String>,
    pub no_mention_channels: HashSet<String>,
    pub mention_patterns: Vec<String>,
    pub http_client: reqwest::Client,
}

/// Start the Slack Events API HTTP webhook server.
///
/// In Socket Mode this server is a complementary listener for event types that
/// slack-morphism does not expose (pin events). When `SLACK_MODE=http` it
/// becomes the sole inbound channel from Slack.
pub async fn start_webhook_server(
    port: u16,
    http_path: String,
    signing_secret: Option<String>,
    js: Arc<JsContext>,
    bot_token: String,
    bot_user_id: Option<String>,
    allow_bots: bool,
    media_max_mb: u64,
    mention_gating: bool,
    mention_gating_channels: HashSet<String>,
    no_mention_channels: HashSet<String>,
    mention_patterns: Vec<String>,
) {
    let state = WebhookState {
        signing_secret,
        js,
        bot_token,
        bot_user_id,
        allow_bots,
        media_max_mb,
        mention_gating,
        mention_gating_channels,
        no_mention_channels,
        mention_patterns,
        http_client: reqwest::Client::new(),
    };

    let app = Router::new()
        .route(&http_path, post(handle_events))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, addr = %addr, "Failed to bind webhook server");
            return;
        }
    };
    tracing::info!(port, path = %http_path, "Slack Events webhook server listening");
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "Webhook server exited with error");
    }
}

async fn handle_events(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // -- 1. Signature verification
    if let Some(ref secret) = state.signing_secret {
        let timestamp = headers
            .get("X-Slack-Request-Timestamp")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let sig_header = headers
            .get("X-Slack-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Reject stale requests (>5 min old).
        if let Ok(ts) = timestamp.parse::<i64>() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if (now - ts).abs() > 300 {
                tracing::warn!("Rejected webhook request: timestamp too old");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }

        let sig_base = format!(
            "v0:{}:{}",
            timestamp,
            std::str::from_utf8(&body).unwrap_or("")
        );
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(sig_base.as_bytes());
        let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

        if expected != sig_header {
            tracing::warn!("Rejected webhook request: invalid Slack signature");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    // -- 2. Route by Content-Type
    let content_type = headers
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if content_type.contains("application/x-www-form-urlencoded") {
        handle_form_payload(&state, &body).await
    } else {
        handle_json_payload(&state, &body).await
    }
}

// -- JSON Events API

async fn handle_json_payload(state: &WebhookState, body: &[u8]) -> axum::response::Response {
    let payload: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to parse webhook body as JSON");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // URL verification challenge.
    if payload["type"] == "url_verification" {
        let challenge = payload["challenge"].as_str().unwrap_or("").to_string();
        return axum::Json(serde_json::json!({ "challenge": challenge })).into_response();
    }

    if payload["type"] == "event_callback" {
        let event = &payload["event"];
        let event_type = event["type"].as_str().unwrap_or("");
        handle_event_callback(state, event_type, event).await;
    }

    StatusCode::OK.into_response()
}

async fn handle_event_callback(
    state: &WebhookState,
    event_type: &str,
    event: &serde_json::Value,
) {
    match event_type {
        "message" => handle_message_event(state, event).await,
        "app_mention" => handle_app_mention_event(state, event).await,
        "reaction_added" => handle_reaction_event(state, event, true).await,
        "reaction_removed" => handle_reaction_event(state, event, false).await,
        "member_joined_channel" => handle_member_event(state, event, true).await,
        "member_left_channel" => handle_member_event(state, event, false).await,
        "channel_created" => handle_channel_event(state, event, ChannelEventKind::Created).await,
        "channel_deleted" => handle_channel_event(state, event, ChannelEventKind::Deleted).await,
        "channel_archive" => handle_channel_event(state, event, ChannelEventKind::Archived).await,
        "channel_unarchive" => handle_channel_event(state, event, ChannelEventKind::Unarchived).await,
        "channel_rename" => handle_channel_event(state, event, ChannelEventKind::Renamed).await,
        "app_home_opened" => handle_app_home_event(state, event).await,
        "link_shared" => handle_link_shared_event(state, event).await,
        "pin_added" => handle_pin_event(state, event, PinEventKind::Added).await,
        "pin_removed" => handle_pin_event(state, event, PinEventKind::Removed).await,
        other => {
            tracing::debug!(event_type = other, "Ignoring unhandled Slack event type");
        }
    }
}

async fn handle_message_event(state: &WebhookState, event: &serde_json::Value) {
    // Drop bot messages unless allow_bots is set.
    if event["bot_id"].is_string() && !state.allow_bots {
        return;
    }

    let subtype = event["subtype"].as_str();

    match subtype {
        Some("message_changed") => {
            let ev = SlackMessageChangedEvent {
                channel: event["channel"].as_str().unwrap_or_default().to_string(),
                ts: event["message"]["ts"].as_str().unwrap_or_default().to_string(),
                event_ts: event["event_ts"].as_str().map(String::from),
                previous_text: event["previous_message"]["text"].as_str().map(String::from),
                new_text: event["message"]["text"].as_str().map(String::from),
                thread_ts: event["message"]["thread_ts"].as_str().map(String::from),
                user: event["message"]["user"].as_str().map(String::from),
            };
            if let Err(e) = publish_message_changed(&state.js, &ev).await {
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
            if let Err(e) = publish_message_deleted(&state.js, &ev).await {
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
            if let Err(e) = publish_thread_broadcast(&state.js, &ev).await {
                tracing::error!(error = %e, "Failed to publish thread_broadcast");
            }
        }
        Some(_) => {} // Ignore other subtypes.
        None => {
            // Regular user message.
            let user = event["user"].as_str().unwrap_or_default().to_string();

            // Drop our own bot messages.
            if let Some(ref bot_uid) = state.bot_user_id {
                if &user == bot_uid {
                    return;
                }
            }

            let channel = event["channel"].as_str().unwrap_or_default().to_string();
            let raw_text = event["text"].as_str().unwrap_or_default().to_string();
            let text = strip_bot_mention(&raw_text, state.bot_user_id.as_deref());

            if text.is_empty() {
                return;
            }

            let ts = event["ts"].as_str().unwrap_or_default().to_string();
            let thread_ts = event["thread_ts"].as_str().map(String::from);

            let session_type = match event["channel_type"].as_str() {
                Some("im") => SessionType::Direct,
                Some("mpim") => SessionType::Group,
                _ => SessionType::Channel,
            };

            // Mention gating.
            if resolve_mention_gating(
                &session_type,
                &channel,
                thread_ts.as_deref(),
                state.mention_gating,
                &state.mention_gating_channels,
                &state.no_mention_channels,
                &text,
                &state.mention_patterns,
            ) {
                if let Some(ref bot_uid) = state.bot_user_id {
                    if !raw_text.contains(&format!("<@{}>", bot_uid)) {
                        return;
                    }
                }
            }

            let session_key = compute_session_key(&session_type, &channel, &user, thread_ts.as_deref());

            // Download file contents.
            let files = extract_and_fetch_files(event, state).await;
            let attachments = extract_attachments(event);
            let display_name = lookup_display_name(&user, &state.bot_token, &state.http_client).await;

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

            if let Err(e) = publish_inbound(&state.js, &inbound).await {
                tracing::error!(error = %e, "Failed to publish inbound message");
            }
        }
    }
}

async fn handle_app_mention_event(state: &WebhookState, event: &serde_json::Value) {
    let user = event["user"].as_str().unwrap_or_default().to_string();
    let channel = event["channel"].as_str().unwrap_or_default().to_string();
    let raw_text = event["text"].as_str().unwrap_or_default().to_string();
    let text = strip_bot_mention(&raw_text, state.bot_user_id.as_deref());

    if text.is_empty() {
        return;
    }

    let ts = event["ts"].as_str().unwrap_or_default().to_string();
    let thread_ts = event["thread_ts"].as_str().map(String::from);
    let session_key = compute_session_key(&SessionType::Channel, &channel, &user, thread_ts.as_deref());

    let files = extract_and_fetch_files(event, state).await;
    let display_name = lookup_display_name(&user, &state.bot_token, &state.http_client).await;

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

    if let Err(e) = publish_inbound(&state.js, &inbound).await {
        tracing::error!(error = %e, "Failed to publish app_mention");
    }
}

async fn handle_reaction_event(state: &WebhookState, event: &serde_json::Value, added: bool) {
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

    if let Err(e) = publish_reaction(&state.js, &ev).await {
        tracing::error!(error = %e, added, "Failed to publish reaction event");
    }
}

async fn handle_member_event(state: &WebhookState, event: &serde_json::Value, joined: bool) {
    let ev = SlackMemberEvent {
        user: event["user"].as_str().unwrap_or_default().to_string(),
        channel: event["channel"].as_str().unwrap_or_default().to_string(),
        channel_type: event["channel_type"].as_str().map(String::from),
        team: event["team"].as_str().map(String::from),
        inviter: event["inviter"].as_str().map(String::from),
        joined,
    };

    if let Err(e) = publish_member(&state.js, &ev).await {
        tracing::error!(error = %e, joined, "Failed to publish member event");
    }
}

async fn handle_channel_event(
    state: &WebhookState,
    event: &serde_json::Value,
    kind: ChannelEventKind,
) {
    // channel_created nests info under "channel"; others are flat.
    let (channel_id, channel_name, user) = if matches!(kind, ChannelEventKind::Created) {
        (
            event["channel"]["id"].as_str().unwrap_or_default().to_string(),
            event["channel"]["name"].as_str().map(String::from),
            event["channel"]["creator"].as_str().map(String::from),
        )
    } else if matches!(kind, ChannelEventKind::Renamed) {
        (
            event["channel"]["id"].as_str().unwrap_or_default().to_string(),
            event["channel"]["name"].as_str().map(String::from),
            None,
        )
    } else {
        (
            event["channel"].as_str().unwrap_or_default().to_string(),
            None,
            event["user"].as_str().map(String::from),
        )
    };

    let ev = SlackChannelEvent { kind, channel_id, channel_name, user };
    if let Err(e) = publish_channel(&state.js, &ev).await {
        tracing::error!(error = %e, "Failed to publish channel event");
    }
}

async fn handle_app_home_event(state: &WebhookState, event: &serde_json::Value) {
    let ev = SlackAppHomeOpenedEvent {
        user: event["user"].as_str().unwrap_or_default().to_string(),
        tab: event["tab"].as_str().unwrap_or_default().to_string(),
        view_id: None,
    };
    if let Err(e) = publish_app_home(&state.js, &ev).await {
        tracing::error!(error = %e, "Failed to publish app_home_opened");
    }
}

async fn handle_link_shared_event(state: &WebhookState, event: &serde_json::Value) {
    let channel = event["channel"].as_str().unwrap_or_default().to_string();
    let message_ts = event["message_ts"].as_str().unwrap_or_default().to_string();
    let urls: Vec<String> = event["links"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|l| l["url"].as_str().map(String::from))
        .collect();

    if !urls.is_empty() {
        let bot_token = state.bot_token.clone();
        let http_client = state.http_client.clone();
        tokio::spawn(async move {
            unfurl_links(&bot_token, &http_client, &channel, &message_ts, &urls).await;
        });
    }
}

async fn handle_pin_event(
    state: &WebhookState,
    event: &serde_json::Value,
    kind: PinEventKind,
) {
    let channel = event["channel_id"]
        .as_str()
        .or_else(|| event["channel"].as_str())
        .unwrap_or_default()
        .to_string();
    let user = event["user"].as_str().unwrap_or_default().to_string();
    let event_ts = event["event_ts"].as_str().unwrap_or_default().to_string();
    let item_ts = event["pin"]["message"]["ts"]
        .as_str()
        .or_else(|| event["item"]["message"]["ts"].as_str())
        .map(String::from);
    let item_type = event["pin"]["type"]
        .as_str()
        .or_else(|| event["item"]["type"].as_str())
        .map(String::from);

    let ev = SlackPinEvent { kind, channel, user, item_ts, item_type, event_ts };
    if let Err(e) = publish_pin(&state.js, &ev).await {
        tracing::error!(error = %e, "Failed to publish pin event");
    }
}

// -- Form-encoded payloads (interactive + slash commands)

async fn handle_form_payload(state: &WebhookState, body: &[u8]) -> axum::response::Response {
    let body_str = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Parse as application/x-www-form-urlencoded.
    let params: std::collections::HashMap<String, String> =
        form_urlencoded::parse(body_str.as_bytes())
            .into_owned()
            .collect();

    if let Some(payload_str) = params.get("payload") {
        // Interactive component (block_actions, view_submission, view_closed).
        handle_interactive_payload(state, payload_str).await
    } else {
        // Slash command.
        handle_slash_command(state, &params).await
    }
}

async fn handle_interactive_payload(
    state: &WebhookState,
    payload_str: &str,
) -> axum::response::Response {
    let payload: serde_json::Value = match serde_json::from_str(payload_str) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to parse interactive payload JSON");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    let interaction_type = payload["type"].as_str().unwrap_or("");

    match interaction_type {
        "block_actions" => {
            let channel_id = payload["channel"]["id"].as_str().map(String::from);
            let user_id = payload["user"]["id"].as_str().unwrap_or_default().to_string();
            let message_ts = payload["message"]["ts"].as_str().map(String::from);
            let trigger_id = payload["trigger_id"].as_str().map(String::from);

            for action in payload["actions"].as_array().unwrap_or(&vec![]).iter() {
                let ev = SlackBlockActionEvent {
                    action_id: action["action_id"].as_str().unwrap_or_default().to_string(),
                    block_id: action["block_id"].as_str().map(String::from),
                    user_id: user_id.clone(),
                    channel_id: channel_id.clone(),
                    message_ts: message_ts.clone(),
                    value: action["value"].as_str().map(String::from),
                    trigger_id: trigger_id.clone(),
                };
                if let Err(e) = publish_block_action(&state.js, &ev).await {
                    tracing::error!(error = %e, "Failed to publish block_action");
                }
            }
        }
        "view_submission" => {
            let user_id = payload["user"]["id"].as_str().unwrap_or_default().to_string();
            let trigger_id = payload["trigger_id"].as_str().unwrap_or_default().to_string();
            let view_id = payload["view"]["id"].as_str().unwrap_or_default().to_string();
            let callback_id = payload["view"]["callback_id"].as_str().map(String::from);
            let values = payload["view"]["state"]["values"].clone();

            let ev = SlackViewSubmissionEvent {
                user_id,
                trigger_id,
                view_id,
                callback_id,
                values,
            };
            if let Err(e) = publish_view_submission(&state.js, &ev).await {
                tracing::error!(error = %e, "Failed to publish view_submission");
            }
        }
        "view_closed" => {
            let user_id = payload["user"]["id"].as_str().unwrap_or_default().to_string();
            let trigger_id = payload["trigger_id"].as_str().unwrap_or_default().to_string();
            let view_id = payload["view"]["id"].as_str().unwrap_or_default().to_string();
            let callback_id = payload["view"]["callback_id"].as_str().map(String::from);

            let ev = SlackViewClosedEvent {
                user_id,
                trigger_id,
                view_id,
                callback_id,
                values: serde_json::Value::Null,
            };
            if let Err(e) = publish_view_closed(&state.js, &ev).await {
                tracing::error!(error = %e, "Failed to publish view_closed");
            }
        }
        other => {
            tracing::debug!(interaction_type = other, "Ignoring unhandled interaction type");
        }
    }

    StatusCode::OK.into_response()
}

async fn handle_slash_command(
    state: &WebhookState,
    params: &std::collections::HashMap<String, String>,
) -> axum::response::Response {
    let command = params.get("command").cloned().unwrap_or_default();
    let text = params.get("text").cloned();
    let user_id = params.get("user_id").cloned().unwrap_or_default();
    let channel_id = params.get("channel_id").cloned().unwrap_or_default();
    let team_id = params.get("team_id").cloned();
    let response_url = params.get("response_url").cloned().unwrap_or_default();
    let trigger_id = params.get("trigger_id").cloned();

    let ev = SlackSlashCommandEvent {
        command,
        text,
        user_id,
        channel_id,
        team_id,
        response_url,
        trigger_id,
    };

    if let Err(e) = publish_slash_command(&state.js, &ev).await {
        tracing::error!(error = %e, "Failed to publish slash command");
    }

    StatusCode::OK.into_response()
}

// -- Helper functions

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

fn strip_bot_mention(text: &str, bot_user_id: Option<&str>) -> String {
    match bot_user_id {
        Some(uid) => text.replace(&format!("<@{}>", uid), "").trim().to_string(),
        None => text.trim().to_string(),
    }
}

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
    if !mention_patterns.is_empty() {
        let text_lower = text.to_lowercase();
        if mention_patterns.iter().any(|p| text_lower.contains(&p.to_lowercase())) {
            return false;
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

async fn extract_and_fetch_files(
    event: &serde_json::Value,
    state: &WebhookState,
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
            .map(|s| s <= state.media_max_mb * 1024 * 1024)
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
            if let Some(url) = file.url_private_download.as_deref().or(file.url_private.as_deref()) {
                match state.http_client.get(url).bearer_auth(&state.bot_token).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(text) = resp.text().await {
                            file.content = Some(text);
                        }
                    }
                    Ok(resp) => tracing::warn!(status = %resp.status(), "Non-success downloading file"),
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
            if let Some(url) = file.url_private_download.as_deref().or(file.url_private.as_deref()) {
                match state.http_client.get(url).bearer_auth(&state.bot_token).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(bytes) = resp.bytes().await {
                            use base64::Engine as _;
                            file.base64_content = Some(
                                base64::engine::general_purpose::STANDARD.encode(&bytes),
                            );
                        }
                    }
                    Ok(resp) => tracing::warn!(status = %resp.status(), "Non-success downloading image"),
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

    resp["user"]["profile"]["display_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| resp["user"]["real_name"].as_str().filter(|s| !s.is_empty()))
        .or_else(|| resp["user"]["name"].as_str().filter(|s| !s.is_empty()))
        .map(String::from)
}

async fn unfurl_links(
    bot_token: &str,
    http_client: &reqwest::Client,
    channel: &str,
    message_ts: &str,
    urls: &[String],
) {
    let mut unfurls = serde_json::Map::new();
    for url in urls {
        unfurls.insert(
            url.clone(),
            serde_json::json!({ "title": url, "text": "" }),
        );
    }

    let body = serde_json::json!({
        "channel": channel,
        "ts": message_ts,
        "unfurls": unfurls,
    });

    let _ = http_client
        .post("https://slack.com/api/chat.unfurl")
        .header("Authorization", format!("Bearer {}", bot_token))
        .json(&body)
        .send()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_session_key_dm() {
        assert_eq!(
            compute_session_key(&SessionType::Direct, "C1", "U1", None),
            Some("slack:dm:U1".to_string())
        );
    }

    #[test]
    fn compute_session_key_channel_no_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Channel, "C1", "U1", None),
            Some("slack:channel:C1".to_string())
        );
    }

    #[test]
    fn compute_session_key_channel_with_thread() {
        assert_eq!(
            compute_session_key(&SessionType::Channel, "C1", "U1", Some("1.0")),
            Some("slack:channel:C1:thread:1.0".to_string())
        );
    }

    #[test]
    fn compute_session_key_group() {
        assert_eq!(
            compute_session_key(&SessionType::Group, "G1", "U1", None),
            Some("slack:group:G1".to_string())
        );
    }

    #[test]
    fn strip_bot_mention_removes_mention() {
        assert_eq!(
            strip_bot_mention("<@UBOT> hello", Some("UBOT")),
            "hello"
        );
    }

    #[test]
    fn strip_bot_mention_no_uid() {
        assert_eq!(strip_bot_mention("  hello  ", None), "hello");
    }

    #[test]
    fn resolve_mention_gating_dm_always_off() {
        assert!(!resolve_mention_gating(
            &SessionType::Direct, "C1", None, true,
            &HashSet::new(), &HashSet::new(), "", &[]
        ));
    }

    #[test]
    fn resolve_mention_gating_thread_always_off() {
        assert!(!resolve_mention_gating(
            &SessionType::Channel, "C1", Some("1.0"), true,
            &HashSet::new(), &HashSet::new(), "", &[]
        ));
    }

    #[test]
    fn resolve_mention_gating_no_mention_channel_overrides() {
        let no_mention: HashSet<String> = vec!["C1".to_string()].into_iter().collect();
        assert!(!resolve_mention_gating(
            &SessionType::Channel, "C1", None, true,
            &HashSet::new(), &no_mention, "", &[]
        ));
    }

    #[test]
    fn resolve_mention_gating_pattern_bypass() {
        let patterns = vec!["hey bot".to_string()];
        assert!(!resolve_mention_gating(
            &SessionType::Channel, "C1", None, true,
            &HashSet::new(), &HashSet::new(), "hey bot help", &patterns
        ));
    }

    #[test]
    fn resolve_mention_gating_falls_back_to_global() {
        assert!(resolve_mention_gating(
            &SessionType::Channel, "C1", None, true,
            &HashSet::new(), &HashSet::new(), "hello", &[]
        ));
    }
}
