use async_nats::Client as NatsClient;
use async_nats::jetstream::Context as JsContext;
use slack_nats::publisher::{
    publish_outbound, publish_reaction_action, publish_stream_append, publish_stream_stop,
};
use slack_types::events::{
    SlackBlockActionEvent, SlackInboundMessage, SlackMessageChangedEvent, SlackMessageDeletedEvent,
    SlackOutboundMessage, SlackPinEvent, SlackReactionAction, SlackReactionEvent,
    SlackSlashCommandEvent, SlackStreamAppendMessage, SlackStreamStopMessage,
    SlackStreamStartRequest, SlackStreamStartResponse, SlackThreadBroadcastEvent,
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

fn get_session_lock(
    ctx: &AgentContext,
    session_key: &str,
) -> Arc<tokio::sync::Mutex<()>> {
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
        content: msg.text.clone(),
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
                fallback_claude_response(claude, &history, &ctx.js, &msg.channel, thread_ts.as_deref()).await
            }
        }
    } else {
        // No Claude configured — echo.
        tracing::warn!("ANTHROPIC_API_KEY not set — echoing message");
        format!("Echo: {}", msg.text)
    };

    // 5. Persist updated history (only if we got a real response).
    if !response_text.is_empty() {
        let mut updated = history;
        updated.push(ConversationMessage {
            role: "assistant".to_string(),
            content: response_text.clone(),
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

    let text = ev.text.as_deref().unwrap_or("").to_string();
    let prompt = format!("{} {}", ev.command, text).trim().to_string();

    let response_text = if let Some(claude) = &ctx.claude {
        let messages = vec![ConversationMessage {
            role: "user".to_string(),
            content: prompt,
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
    post_response_url(&ev.response_url, &response_text).await;
}

/// POST a delayed response to a Slack `response_url`.
async fn post_response_url(response_url: &str, text: &str) {
    #[derive(serde::Serialize)]
    struct SlackResponseUrlPayload<'a> {
        text: &'a str,
        response_type: &'a str,
    }

    let client = reqwest::Client::new();
    let payload = SlackResponseUrlPayload {
        text,
        response_type: "in_channel",
    };
    if let Err(e) = client.post(response_url).json(&payload).send().await {
        tracing::error!(error = %e, url = %response_url, "Failed to POST to response_url");
    }
}

// ── Remaining event handlers (log-only for now) ───────────────────────────────

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

pub async fn handle_message_changed(ev: SlackMessageChangedEvent) {
    tracing::info!(
        channel = %ev.channel,
        ts = %ev.ts,
        previous_text = ?ev.previous_text,
        new_text = ?ev.new_text,
        "Received message_changed event"
    );
}

pub async fn handle_message_deleted(ev: SlackMessageDeletedEvent) {
    tracing::info!(
        channel = %ev.channel,
        deleted_ts = %ev.deleted_ts,
        "Received message_deleted event"
    );
}

pub async fn handle_thread_broadcast(ev: SlackThreadBroadcastEvent) {
    tracing::info!(
        channel = %ev.channel,
        user = %ev.user,
        thread_ts = %ev.thread_ts,
        "Received thread_broadcast event"
    );
}

pub async fn handle_pin(ev: SlackPinEvent) {
    tracing::info!(
        channel = %ev.channel,
        user = %ev.user,
        added = %ev.added,
        item_ts = ?ev.item_ts,
        "Received pin event"
    );
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
