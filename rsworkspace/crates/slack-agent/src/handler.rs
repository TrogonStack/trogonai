use async_nats::Client as NatsClient;
use async_nats::jetstream::Context as JsContext;
use slack_nats::publisher::{
    publish_outbound, publish_reaction_action, publish_stream_append, publish_stream_stop,
};
use slack_types::events::{
    SessionType, SlackBlockActionEvent, SlackChannelEvent, SlackFile, SlackInboundMessage,
    SlackMemberEvent, SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackOutboundMessage,
    SlackReactionAction, SlackReactionEvent, SlackSlashCommandEvent, SlackStreamAppendMessage,
    SlackStreamStartRequest, SlackStreamStartResponse, SlackStreamStopMessage,
    SlackThreadBroadcastEvent,
};
use slack_types::subjects::SLACK_OUTBOUND_STREAM_START;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{ReplyToMode, SlackAgentConfig};
use crate::llm::ClaudeClient;
use crate::memory::{ConversationMemory, ConversationMessage};

/// Timeout for the `stream.start` Core NATS request/reply round-trip.
const STREAM_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared state passed to every handler call.
pub struct AgentContext {
    pub js: Arc<JsContext>,
    /// Raw Core NATS client — used for request/reply (stream.start).
    pub nats: NatsClient,
    pub claude: Option<ClaudeClient>,
    pub memory: ConversationMemory,
    pub config: SlackAgentConfig,
    /// Per-session mutex to serialise history read/write for the same session.
    pub session_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Reusable HTTP client for Slack response_url webhooks.
    pub http_client: reqwest::Client,
}

// ── Helper: resolve thread_ts ─────────────────────────────────────────────────

pub fn resolve_reply_thread_ts(
    mode: &ReplyToMode,
    message_ts: &str,
    incoming_thread_ts: Option<&str>,
) -> Option<String> {
    match mode {
        ReplyToMode::Off => None,
        ReplyToMode::First => incoming_thread_ts
            .map(str::to_string)
            .or_else(|| Some(message_ts.to_string())),
        ReplyToMode::All => Some(message_ts.to_string()),
    }
}

// ── Helper: ack reaction ──────────────────────────────────────────────────────

async fn ack_reaction(js: &JsContext, channel: &str, ts: &str, emoji: &str, add: bool) {
    let action = SlackReactionAction {
        channel: channel.to_string(),
        ts: ts.to_string(),
        reaction: emoji.to_string(),
        add,
    };
    if let Err(e) = publish_reaction_action(js, &action).await {
        tracing::warn!(error = %e, add, emoji, "Failed to publish ack reaction");
    }
}

// ── Helper: request stream.start via Core NATS request/reply ─────────────────

async fn request_stream_start(
    nats: &NatsClient,
    channel: &str,
    thread_ts: Option<&str>,
) -> Result<SlackStreamStartResponse, String> {
    let req = SlackStreamStartRequest {
        channel: channel.to_string(),
        thread_ts: thread_ts.map(str::to_string),
        initial_text: Some("…".to_string()),
    };
    let payload = serde_json::to_vec(&req).map_err(|e| format!("serialize: {e}"))?;
    let msg = nats
        .request(SLACK_OUTBOUND_STREAM_START, payload.into())
        .await
        .map_err(|e| format!("request: {e}"))?;
    serde_json::from_slice::<SlackStreamStartResponse>(&msg.payload)
        .map_err(|e| format!("deserialize reply: {e}"))
}

// ── Helper: get or create per-session lock ────────────────────────────────────

fn get_session_lock(ctx: &AgentContext, session_key: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = ctx.session_locks.lock().unwrap();
    locks
        .entry(session_key.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

// ── Main inbound handler ──────────────────────────────────────────────────────

/// Handles a regular inbound message.
///
/// Flow:
/// 1. Add ack reaction (if configured).
/// 2. Acquire per-session lock (serialises concurrent messages for the same session).
/// 3. Load conversation history from NATS KV.
/// 4. Request a streaming placeholder via `stream.start` (Core NATS req/reply, 10 s timeout).
/// 5. Call Claude, streaming chunks → `stream.append` every ~80 new chars.
/// 6. Finalize with `stream.stop`.
/// 7. Save updated history to NATS KV.
/// 8. Remove ack reaction.
pub async fn handle_inbound(msg: SlackInboundMessage, ctx: Arc<AgentContext>) {
    tracing::info!(
        channel = %msg.channel,
        user = %msg.user,
        text = %msg.text,
        session_key = ?msg.session_key,
        "Processing inbound message"
    );

    let session_key = msg
        .session_key
        .as_deref()
        .unwrap_or(&msg.channel)
        .to_string();

    // Prune session locks whose Arc is held only by the map (no task is using them).
    {
        let mut locks = ctx.session_locks.lock().unwrap();
        locks.retain(|_, arc| Arc::strong_count(arc) > 1);
    }

    let thread_ts =
        resolve_reply_thread_ts(&ctx.config.reply_to_mode, &msg.ts, msg.thread_ts.as_deref());

    // 1. Add ack reaction.
    if let Some(emoji) = &ctx.config.ack_reaction {
        ack_reaction(&ctx.js, &msg.channel, &msg.ts, emoji, true).await;
    }

    // 2. Acquire per-session lock so parallel messages don't corrupt history.
    let session_lock = get_session_lock(&ctx, &session_key);
    let _session_guard = session_lock.lock().await;

    // 3. Load conversation history.
    let mut history = ctx.memory.load(&session_key).await;
    history.push(ConversationMessage {
        role: "user".to_string(),
        content: build_message_content(&msg.text, &msg.files),
        ts: Some(msg.ts.clone()),
    });

    // 4. Determine the response text (streaming or fallback).
    let response_text = if let Some(claude) = &ctx.claude {
        // 4a. Open a streaming placeholder on Slack (with timeout).
        let stream_ref = match tokio::time::timeout(
            STREAM_START_TIMEOUT,
            request_stream_start(&ctx.nats, &msg.channel, thread_ts.as_deref()),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::warn!("stream.start timed out after {STREAM_START_TIMEOUT:?}");
                Err("stream.start timed out".to_string())
            }
        };

        match stream_ref {
            Ok(stream_start) => {
                // 4b. Stream Claude response; publish stream.append periodically.
                match claude.stream_response(history.clone()).await {
                    Ok((mut rx, handle)) => {
                        let mut accumulated = String::new();
                        let mut last_published_len: usize = 0;

                        while let Some(chunk) = rx.recv().await {
                            accumulated.push_str(&chunk);

                            // Publish stream.append every 80 new chars or on sentence end.
                            let new_len = accumulated.len();
                            let is_sentence_end = chunk.ends_with('.')
                                || chunk.ends_with('!')
                                || chunk.ends_with('?')
                                || chunk.ends_with('\n');

                            if new_len - last_published_len >= 80 || is_sentence_end {
                                let append = SlackStreamAppendMessage {
                                    channel: stream_start.channel.clone(),
                                    ts: stream_start.ts.clone(),
                                    text: accumulated.clone(),
                                };
                                if let Err(e) = publish_stream_append(&ctx.js, &append).await {
                                    tracing::warn!(error = %e, "Failed to publish stream.append");
                                }
                                last_published_len = new_len;
                            }
                        }

                        // Await the handle to get the full text (and propagate errors).
                        let final_text = match handle.await {
                            Ok(Ok(text)) => text,
                            Ok(Err(e)) => {
                                tracing::error!(error = %e, "Claude stream task error");
                                accumulated
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Claude stream task panicked");
                                accumulated
                            }
                        };

                        // 4c. Finalize with stream.stop.
                        let stop = SlackStreamStopMessage {
                            channel: stream_start.channel.clone(),
                            ts: stream_start.ts.clone(),
                            final_text: final_text.clone(),
                            blocks: None,
                        };
                        if let Err(e) = publish_stream_stop(&ctx.js, &stop).await {
                            tracing::error!(error = %e, "Failed to publish stream.stop");
                        }

                        final_text
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Claude streaming failed");
                        let fallback = format!("Sorry, I encountered an error: {e}");
                        // Still close the stream gracefully.
                        let stop = SlackStreamStopMessage {
                            channel: stream_start.channel.clone(),
                            ts: stream_start.ts.clone(),
                            final_text: fallback.clone(),
                            blocks: None,
                        };
                        let _ = publish_stream_stop(&ctx.js, &stop).await;
                        fallback
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "stream.start failed — falling back to chat.postMessage"
                );
                // Fallback: call Claude without streaming.
                fallback_claude_response(
                    claude,
                    &history,
                    &ctx.js,
                    &msg.channel,
                    thread_ts.as_deref(),
                )
                .await
            }
        }
    } else {
        // No Claude configured — echo back to Slack.
        tracing::warn!("ANTHROPIC_API_KEY not set — echoing message");
        let echo = format!("Echo: {}", msg.text);
        let outbound = SlackOutboundMessage {
            channel: msg.channel.clone(),
            text: echo.clone(),
            thread_ts,
            blocks: None,
            media_url: None,
            username: None,
            icon_url: None,
        };
        if let Err(e) = publish_outbound(&ctx.js, &outbound).await {
            tracing::error!(error = %e, "Failed to publish echo outbound");
        }
        echo
    };

    // 5. Persist updated history (only if we got a real response).
    if !response_text.is_empty() {
        let mut updated = history;
        updated.push(ConversationMessage {
            role: "assistant".to_string(),
            content: response_text.clone(),
            ts: None,
        });
        ctx.memory.save(&session_key, &updated).await;
    }

    // 6. Remove ack reaction.
    if let Some(emoji) = &ctx.config.ack_reaction {
        ack_reaction(&ctx.js, &msg.channel, &msg.ts, emoji, false).await;
    }
}

/// Fallback: call Claude without streaming, send response as a regular message.
async fn fallback_claude_response(
    claude: &ClaudeClient,
    history: &[ConversationMessage],
    js: &JsContext,
    channel: &str,
    thread_ts: Option<&str>,
) -> String {
    match claude.stream_response(history.to_vec()).await {
        Ok((mut rx, handle)) => {
            // Drain the channel (stream runs in background task).
            while rx.recv().await.is_some() {}
            match handle.await {
                Ok(Ok(text)) => {
                    let outbound = SlackOutboundMessage {
                        channel: channel.to_string(),
                        text: text.clone(),
                        thread_ts: thread_ts.map(str::to_string),
                        blocks: None,
                        media_url: None,
                        username: None,
                        icon_url: None,
                    };
                    if let Err(e) = publish_outbound(js, &outbound).await {
                        tracing::error!(error = %e, "Failed to publish fallback outbound");
                    }
                    text
                }
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "Claude task error in fallback");
                    "Sorry, I encountered an error. Please try again.".to_string()
                }
                Err(e) => {
                    tracing::error!(error = %e, "Claude task panicked in fallback");
                    "Sorry, I encountered an error. Please try again.".to_string()
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Claude stream_response failed in fallback");
            "Sorry, I encountered an error. Please try again.".to_string()
        }
    }
}

// ── Slash command handler ─────────────────────────────────────────────────────

/// Handles a slash command.
///
/// Calls Claude with the command text, then POSTs the response to the Slack
/// `response_url` (webhook) — this avoids needing an active session for
/// commands triggered outside a channel context.
pub async fn handle_slash_command(ev: SlackSlashCommandEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        command = %ev.command,
        user_id = %ev.user_id,
        channel_id = %ev.channel_id,
        text = ?ev.text,
        "Received slash command"
    );

    let text = ev.text.as_deref().unwrap_or("").trim().to_string();

    // Special built-in: /command clear — wipe conversation history for this session.
    if text.eq_ignore_ascii_case("clear") {
        // Derive session key from channel_id prefix (standard Slack conventions):
        //   D… = IM / direct message  → keyed by user
        //   G… = group DM (MPIM)      → keyed by channel
        //   C… = regular channel      → keyed by channel
        let session_key = if ev.channel_id.starts_with('D') {
            format!("slack:dm:{}", ev.user_id)
        } else if ev.channel_id.starts_with('G') {
            format!("slack:group:{}", ev.channel_id)
        } else {
            format!("slack:channel:{}", ev.channel_id)
        };
        ctx.memory.clear(&session_key).await;
        // Also evict the per-session lock so memory is fully reset.
        ctx.session_locks.lock().unwrap().remove(&session_key);
        tracing::info!(channel = %ev.channel_id, "Conversation history cleared via slash command");
        post_response_url(
            &ctx.http_client,
            &ev.response_url,
            "Conversation history cleared.",
        )
        .await;
        return;
    }

    let prompt = format!("{} {}", ev.command, text).trim().to_string();

    let response_text = if let Some(claude) = &ctx.claude {
        let messages = vec![ConversationMessage {
            role: "user".to_string(),
            content: prompt,
            ts: None,
        }];
        match claude.stream_response(messages).await {
            Ok((mut rx, handle)) => {
                while rx.recv().await.is_some() {}
                match handle.await {
                    Ok(Ok(text)) => text,
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "Claude error for slash command");
                        format!("Error processing command: {e}")
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Claude task panicked");
                        "Internal error.".to_string()
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Claude unavailable for slash command");
                format!("Error: {e}")
            }
        }
    } else {
        format!("Command `{}` received: {}", ev.command, text)
    };

    // POST the response to the Slack response_url webhook.
    post_response_url(&ctx.http_client, &ev.response_url, &response_text).await;
}

/// POST a delayed response to a Slack `response_url`.
async fn post_response_url(client: &reqwest::Client, response_url: &str, text: &str) {
    #[derive(serde::Serialize)]
    struct SlackResponseUrlPayload<'a> {
        text: &'a str,
        response_type: &'a str,
    }

    let payload = SlackResponseUrlPayload {
        text,
        response_type: "in_channel",
    };
    if let Err(e) = client.post(response_url).json(&payload).send().await {
        tracing::error!(error = %e, url = %response_url, "Failed to POST to response_url");
    }
}

// ── Session key derivation from raw channel/user fields ──────────────────────

/// Derive a session key from a raw Slack channel ID and optional user/thread
/// fields. Returns `None` when the key cannot be determined (e.g. a DM event
/// without a user_id).
fn derive_session_key_for_event(
    channel: &str,
    user: Option<&str>,
    thread_ts: Option<&str>,
) -> Option<String> {
    if channel.starts_with('D') {
        // DM: session is per-user, not per-channel
        Some(format!("slack:dm:{}", user?))
    } else if channel.starts_with('G') {
        Some(match thread_ts {
            Some(tts) => format!("slack:group:{}:thread:{}", channel, tts),
            None => format!("slack:group:{}", channel),
        })
    } else {
        Some(match thread_ts {
            Some(tts) => format!("slack:channel:{}:thread:{}", channel, tts),
            None => format!("slack:channel:{}", channel),
        })
    }
}

// ── Remaining event handlers ──────────────────────────────────────────────────

pub async fn handle_reaction(ev: SlackReactionEvent) {
    tracing::info!(
        reaction = %ev.reaction,
        user = %ev.user,
        added = %ev.added,
        channel = ?ev.channel,
        item_ts = ?ev.item_ts,
        "Received reaction event"
    );
}

pub async fn handle_message_changed(ev: SlackMessageChangedEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        channel = %ev.channel,
        ts = %ev.ts,
        new_text = ?ev.new_text,
        "Received message_changed event"
    );

    let new_text = match ev.new_text.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => return,
    };

    let Some(session_key) =
        derive_session_key_for_event(&ev.channel, ev.user.as_deref(), ev.thread_ts.as_deref())
    else {
        tracing::debug!(
            channel = %ev.channel,
            "message_changed: cannot derive session key (DM without user_id?), skipping"
        );
        return;
    };

    let mut history = ctx.memory.load(&session_key).await;
    if let Some(entry) = history
        .iter_mut()
        .find(|m| m.role == "user" && m.ts.as_deref() == Some(ev.ts.as_str()))
    {
        entry.content = new_text;
        ctx.memory.save(&session_key, &history).await;
        tracing::debug!(session_key = %session_key, ts = %ev.ts, "Updated history entry for edited message");
    } else {
        tracing::debug!(session_key = %session_key, ts = %ev.ts, "message_changed: no matching history entry");
    }
}

pub async fn handle_message_deleted(ev: SlackMessageDeletedEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        channel = %ev.channel,
        deleted_ts = %ev.deleted_ts,
        "Received message_deleted event"
    );

    // DM session keys require the user_id, which is absent from delete events.
    if ev.channel.starts_with('D') {
        tracing::debug!(channel = %ev.channel, "message_deleted in DM — skipping (no user_id in event)");
        return;
    }

    let Some(session_key) =
        derive_session_key_for_event(&ev.channel, None, ev.thread_ts.as_deref())
    else {
        return;
    };

    let mut history = ctx.memory.load(&session_key).await;
    let before = history.len();
    history.retain(|m| m.ts.as_deref() != Some(ev.deleted_ts.as_str()));

    if history.len() < before {
        ctx.memory.save(&session_key, &history).await;
        tracing::debug!(session_key = %session_key, ts = %ev.deleted_ts, "Removed deleted message from history");
    } else {
        tracing::debug!(session_key = %session_key, ts = %ev.deleted_ts, "message_deleted: no matching history entry");
    }
}

pub async fn handle_thread_broadcast(ev: SlackThreadBroadcastEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        channel = %ev.channel,
        user = %ev.user,
        thread_ts = %ev.thread_ts,
        "Received thread_broadcast event — routing to inbound handler"
    );
    // A thread broadcast is a thread reply also sent to the channel.
    // Route it through the normal inbound pipeline so Claude can respond.
    let session_key = format!("slack:channel:{}:thread:{}", ev.channel, ev.thread_ts);
    let inbound = SlackInboundMessage {
        channel: ev.channel,
        user: ev.user,
        text: ev.text,
        ts: ev.ts,
        event_ts: ev.event_ts,
        thread_ts: Some(ev.thread_ts),
        parent_user_id: None,
        session_type: SessionType::Channel,
        source: Some("thread_broadcast".to_string()),
        session_key: Some(session_key),
        files: vec![],
        attachments: vec![],
    };
    handle_inbound(inbound, ctx).await;
}

pub async fn handle_block_action(ev: SlackBlockActionEvent) {
    tracing::info!(
        action_id = %ev.action_id,
        user_id = %ev.user_id,
        channel_id = ?ev.channel_id,
        message_ts = ?ev.message_ts,
        value = ?ev.value,
        "Received block action"
    );
}

pub async fn handle_member(ev: SlackMemberEvent) {
    tracing::info!(
        user = %ev.user,
        channel = %ev.channel,
        joined = %ev.joined,
        "Received member event"
    );
}

pub async fn handle_channel(ev: SlackChannelEvent) {
    tracing::info!(
        channel_id = %ev.channel_id,
        channel_name = ?ev.channel_name,
        kind = ?ev.kind,
        "Received channel event"
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the content string sent to Claude, appending file metadata when present.
///
/// Even though we can't download private Slack files from the agent, giving
/// Claude the file names and MIME types lets it acknowledge attachments and
/// ask the user for context if needed.
fn build_message_content(text: &str, files: &[SlackFile]) -> String {
    if files.is_empty() {
        return text.to_string();
    }
    let mut content = text.to_string();
    content.push_str("\n\n[Attached files:");
    for f in files {
        let name = f.name.as_deref().unwrap_or("unknown");
        let mime = f.mimetype.as_deref().unwrap_or("unknown type");
        content.push_str(&format!("\n- {} ({})", name, mime));
    }
    content.push(']');
    content
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReplyToMode;

    // ── derive_session_key_for_event ──────────────────────────────────────────

    #[test]
    fn session_key_dm_with_user() {
        assert_eq!(
            derive_session_key_for_event("D123", Some("U1"), None),
            Some("slack:dm:U1".to_string())
        );
    }

    #[test]
    fn session_key_dm_without_user_returns_none() {
        assert_eq!(derive_session_key_for_event("D123", None, None), None);
    }

    #[test]
    fn session_key_group_no_thread() {
        assert_eq!(
            derive_session_key_for_event("G123", Some("U1"), None),
            Some("slack:group:G123".to_string())
        );
    }

    #[test]
    fn session_key_group_with_thread() {
        assert_eq!(
            derive_session_key_for_event("G123", None, Some("1.0")),
            Some("slack:group:G123:thread:1.0".to_string())
        );
    }

    #[test]
    fn session_key_channel_no_thread() {
        assert_eq!(
            derive_session_key_for_event("C123", None, None),
            Some("slack:channel:C123".to_string())
        );
    }

    #[test]
    fn session_key_channel_with_thread() {
        assert_eq!(
            derive_session_key_for_event("C123", None, Some("9.0")),
            Some("slack:channel:C123:thread:9.0".to_string())
        );
    }

    // ── resolve_reply_thread_ts ───────────────────────────────────────────────

    #[test]
    fn reply_to_off_returns_none() {
        assert_eq!(
            resolve_reply_thread_ts(&ReplyToMode::Off, "1.0", Some("2.0")),
            None
        );
    }

    #[test]
    fn reply_to_first_uses_incoming_thread_ts() {
        assert_eq!(
            resolve_reply_thread_ts(&ReplyToMode::First, "1.0", Some("2.0")),
            Some("2.0".to_string())
        );
    }

    #[test]
    fn reply_to_first_falls_back_to_message_ts() {
        assert_eq!(
            resolve_reply_thread_ts(&ReplyToMode::First, "1.0", None),
            Some("1.0".to_string())
        );
    }

    #[test]
    fn reply_to_all_always_uses_message_ts() {
        assert_eq!(
            resolve_reply_thread_ts(&ReplyToMode::All, "1.0", Some("2.0")),
            Some("1.0".to_string())
        );
        assert_eq!(
            resolve_reply_thread_ts(&ReplyToMode::All, "1.0", None),
            Some("1.0".to_string())
        );
    }

    // ── build_message_content ─────────────────────────────────────────────────

    #[test]
    fn no_files_returns_text_unchanged() {
        assert_eq!(build_message_content("hello", &[]), "hello");
    }

    #[test]
    fn files_appended_to_content() {
        let files = vec![SlackFile {
            id: Some("F1".into()),
            name: Some("photo.png".into()),
            mimetype: Some("image/png".into()),
            url_private: None,
            url_private_download: None,
            size: None,
        }];
        let result = build_message_content("look at this", &files);
        assert!(result.starts_with("look at this"));
        assert!(result.contains("photo.png"));
        assert!(result.contains("image/png"));
    }

    #[test]
    fn files_with_unknown_fields_use_fallback() {
        let files = vec![SlackFile {
            id: None,
            name: None,
            mimetype: None,
            url_private: None,
            url_private_download: None,
            size: None,
        }];
        let result = build_message_content("hi", &files);
        assert!(result.contains("unknown"));
        assert!(result.contains("unknown type"));
    }
}
