use async_nats::Client as NatsClient;
use async_nats::jetstream::Context as JsContext;
use slack_nats::publisher::{
    publish_delete_file, publish_ephemeral_message, publish_outbound, publish_reaction_action,
    publish_set_status, publish_set_suggested_prompts, publish_stream_append, publish_stream_stop,
    publish_upload_request, publish_view_open, publish_view_publish,
};
use slack_types::events::{
    PinEventKind, SessionType, SlackAppHomeOpenedEvent, SlackAttachment, SlackBlockActionEvent,
    SlackChannelEvent, SlackDeleteFile, SlackEphemeralMessage, SlackFile, SlackInboundMessage,
    SlackMemberEvent, SlackMessageChangedEvent, SlackMessageDeletedEvent, SlackOutboundMessage,
    SlackPinEvent, SlackReactionAction, SlackReactionEvent, SlackReadRepliesRequest,
    SlackSetStatusRequest, SlackSetSuggestedPromptsRequest, SlackSlashCommandEvent,
    SlackStreamAppendMessage, SlackStreamStartRequest, SlackStreamStartResponse,
    SlackStreamStopMessage, SlackThreadBroadcastEvent, SlackUploadRequest, SlackViewClosedEvent,
    SlackViewOpenRequest, SlackViewPublishRequest, SlackViewSubmissionEvent,
};
use slack_types::subjects::SLACK_OUTBOUND_STREAM_START;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{DmPolicy, ReplyToMode, SlackAgentConfig};
use crate::llm::ClaudeClient;
use crate::memory::{ConversationMemory, ConversationMessage};
use crate::user_settings::{UserSettings, UserSettingsStore};
use crate::views::{build_app_home_view, build_settings_modal};

/// Timeout for the `stream.start` Core NATS request/reply round-trip.
const STREAM_START_TIMEOUT: Duration = Duration::from_secs(10);

// ── Per-user rate limiter ─────────────────────────────────────────────────────

/// Tracks per-user message counts within a sliding 60-second window.
pub struct UserRateLimiter {
    limit: u32, // 0 = disabled
    /// Maps user_id -> list of Instant timestamps within the current window.
    window: Mutex<HashMap<String, Vec<Instant>>>,
}

impl UserRateLimiter {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            window: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if the user is allowed (within limit), false if rate-limited.
    /// Also cleans up timestamps older than 60 seconds.
    fn check_and_record(&self, user_id: &str) -> bool {
        if self.limit == 0 {
            return true;
        }
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut map = self.window.lock().unwrap();
        let timestamps = map.entry(user_id.to_string()).or_default();
        // Remove entries older than 60 seconds.
        timestamps.retain(|t| now.duration_since(*t) < window);
        if timestamps.len() >= self.limit as usize {
            return false;
        }
        timestamps.push(now);
        true
    }
}

/// Shared state passed to every handler call.
pub struct AgentContext {
    pub js: Arc<JsContext>,
    /// Raw Core NATS client — used for request/reply (stream.start).
    pub nats: NatsClient,
    pub claude: Option<ClaudeClient>,
    pub memory: ConversationMemory,
    /// Per-user settings (model, system prompt) backed by NATS KV.
    pub user_settings: UserSettingsStore,
    pub config: SlackAgentConfig,
    /// Resolved system prompt (file contents take precedence over inline text).
    pub base_system_prompt: Option<String>,
    /// Per-session mutex to serialise history read/write for the same session.
    pub session_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Reusable HTTP client for Slack response_url webhooks.
    pub http_client: reqwest::Client,
    /// Per-user sliding-window rate limiter for inbound messages.
    pub user_rate_limiter: UserRateLimiter,
    /// Debounce window in milliseconds (0 = disabled).
    pub debounce_ms: u64,
    /// Per-session pending debounce task handles; keyed by session key.
    pub session_debounce: tokio::sync::Mutex<HashMap<String, tokio::task::AbortHandle>>,
    /// Semaphore limiting concurrent Claude calls. `None` = unlimited.
    pub claude_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Cache mapping channel:thread_ts -> file_id for uploaded responses.
    /// Used to delete the file on regenerate (arrows_counterclockwise).
    pub file_id_cache: Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>,
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

// ── Helper: publish a fresh App Home view ────────────────────────────────────

async fn refresh_app_home(ctx: &AgentContext, user_id: &str) {
    let settings = ctx.user_settings.load(user_id).await;
    let session_key = format!("slack:dm:{}", user_id);
    let history = ctx.memory.load(&session_key).await;
    let effective_model = settings.model.as_deref().unwrap_or(&ctx.config.claude_model);
    let view = build_app_home_view(
        effective_model,
        history.len(),
        settings.system_prompt.as_deref(),
    );
    let req = SlackViewPublishRequest {
        user_id: user_id.to_string(),
        view,
    };
    if let Err(e) = publish_view_publish(&ctx.js, &req).await {
        tracing::warn!(error = %e, user = %user_id, "Failed to refresh App Home view");
    }
}

// ── Main inbound handler ──────────────────────────────────────────────────────

/// Core processing logic for an inbound message.
///
/// Contains all Claude call + history management after the early checks.
/// Invoked either directly (debounce disabled) or after the debounce delay.
async fn process_inbound(msg: SlackInboundMessage, ctx: Arc<AgentContext>) {
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

    // DM pairing: override session_key to share history with the paired channel.
    let session_key = if matches!(msg.session_type, SessionType::Direct) {
        if let Some(ref pair_channel) = ctx.config.dm_pair_channel {
            format!("slack:channel:{}", pair_channel)
        } else {
            session_key
        }
    } else {
        session_key
    };

    // Prune session locks whose Arc is held only by the map (no task is using them).
    {
        let mut locks = ctx.session_locks.lock().unwrap();
        locks.retain(|_, arc| Arc::strong_count(arc) > 1);
    }

    // Extract manual reply-target directive; compute effective thread_ts.
    let (effective_text, reply_target_override) = extract_reply_target(&msg.text, &msg.ts);
    let effective_reply_mode = match msg.session_type {
        SessionType::Direct => ctx
            .config
            .reply_to_mode_dm
            .as_ref()
            .unwrap_or(&ctx.config.reply_to_mode),
        SessionType::Group => ctx
            .config
            .reply_to_mode_group
            .as_ref()
            .unwrap_or(&ctx.config.reply_to_mode),
        SessionType::Channel => &ctx.config.reply_to_mode,
    };
    let effective_thread_ts = reply_target_override.or_else(|| {
        resolve_reply_thread_ts(effective_reply_mode, &msg.ts, msg.thread_ts.as_deref())
    });

    // Load per-user settings to optionally override model / system prompt.
    let user_settings = ctx.user_settings.load(&msg.user).await;

    // Per-channel system prompt: takes precedence over global base, but not user setting.
    let channel_prompt: Option<String> = ctx.config.channel_system_prompts.get(&msg.channel).cloned();

    let user_claude: Option<ClaudeClient> =
        if user_settings.model.is_some() || user_settings.system_prompt.is_some() || channel_prompt.is_some() {
            ctx.config.anthropic_api_key.as_ref().map(|key| {
                let model = user_settings
                    .model
                    .as_deref()
                    .unwrap_or(&ctx.config.claude_model)
                    .to_string();
                let prompt = user_settings
                    .system_prompt
                    .clone()
                    .or(channel_prompt)
                    .or_else(|| ctx.base_system_prompt.clone());
                ClaudeClient::new(key.clone(), model, ctx.config.claude_max_tokens, prompt)
            })
        } else {
            None
        };
    let claude = user_claude.as_ref().or(ctx.claude.as_ref());

    // 1. Add ack reaction.
    if let Some(emoji) = &ctx.config.ack_reaction {
        ack_reaction(&ctx.js, &msg.channel, &msg.ts, emoji, true).await;
    }

    // 2. Acquire per-session lock so parallel messages don't corrupt history.
    let session_lock = get_session_lock(&ctx, &session_key);
    let _session_guard = session_lock.lock().await;

    // 3. Load conversation history.
    let mut history = ctx.memory.load(&session_key).await;

    // 3a. Seed thread history from parent channel session when starting a new thread.
    if history.is_empty()
        && msg.thread_ts.is_some()
        && ctx.config.thread_initial_history_limit > 0
    {
        let parent_key = format!("slack:channel:{}", msg.channel);
        let parent_history = ctx.memory.load(&parent_key).await;
        if !parent_history.is_empty() {
            let limit = ctx.config.thread_initial_history_limit;
            let start = parent_history.len().saturating_sub(limit);
            history = parent_history[start..].to_vec();
            tracing::debug!(
                count = history.len(),
                "Seeded thread history from parent channel session"
            );
        }
    }

    // 3b. Seed initial context from Slack history if session is fresh.
    let history = if history.is_empty()
        && ctx.config.slack_seed_history_on_start > 0
        && matches!(msg.session_type, SessionType::Channel | SessionType::Group)
    {
        // Determine whether this message is inside a thread (thread_ts set and
        // different from ts).  Thread messages use conversations.replies; channel
        // root messages use conversations.history.
        let is_thread_reply = msg
            .thread_ts
            .as_deref()
            .map(|tts| tts != msg.ts.as_str())
            .unwrap_or(false);

        let messages_result: Result<Vec<slack_types::events::SlackReadMessage>, String> =
            if is_thread_reply {
                use slack_nats::publisher::request_read_replies;
                let thread_ts = msg.thread_ts.as_deref().unwrap(); // safe: is_thread_reply=true
                let req = SlackReadRepliesRequest {
                    channel: msg.channel.clone(),
                    ts: thread_ts.to_string(),
                    limit: Some(ctx.config.slack_seed_history_on_start as u32),
                    oldest: None,
                    latest: Some(msg.ts.clone()), // only messages before current
                };
                match request_read_replies(&ctx.nats, &req).await {
                    Ok(resp) if resp.ok => Ok(resp.messages),
                    Ok(resp) => Err(resp.error.unwrap_or_else(|| "unknown error".to_string())),
                    Err(e) => Err(e.to_string()),
                }
            } else {
                use slack_nats::publisher::request_read_messages;
                use slack_types::events::SlackReadMessagesRequest;
                let req = SlackReadMessagesRequest {
                    channel: msg.channel.clone(),
                    limit: Some(ctx.config.slack_seed_history_on_start as u32),
                    oldest: None,
                    latest: Some(msg.ts.clone()), // only messages before current
                };
                match request_read_messages(&ctx.nats, &req).await {
                    Ok(resp) if resp.ok => Ok(resp.messages),
                    Ok(resp) => Err(resp.error.unwrap_or_else(|| "unknown error".to_string())),
                    Err(e) => Err(e.to_string()),
                }
            };

        match messages_result {
            Ok(messages) if !messages.is_empty() => {
                let seeded: Vec<ConversationMessage> = messages
                    .iter()
                    .rev() // API returns newest-first; reverse to chronological
                    .filter_map(|m| {
                        m.text.as_ref().filter(|t| !t.is_empty()).map(|text| {
                            let role = if m.bot_id.is_some() {
                                "assistant"
                            } else {
                                "user"
                            };
                            ConversationMessage {
                                role: role.to_string(),
                                content: text.clone(),
                                ts: Some(m.ts.clone()),
                                images: vec![],
                            }
                        })
                    })
                    .collect();
                tracing::debug!(
                    count = seeded.len(),
                    channel = %msg.channel,
                    is_thread_reply,
                    "Seeded session history from Slack"
                );
                seeded
            }
            Ok(_) => history, // empty messages list
            Err(e) => {
                tracing::warn!(error = %e, "Failed to seed history from Slack");
                history
            }
        }
    } else {
        history
    };
    let mut history = history;

    // Track whether the session is fresh (no prior turns) before appending the new message.
    let history_was_empty = history.is_empty();

    // Prefix message with the user's display name so Claude knows who's speaking.
    let content_text = match msg.display_name.as_deref() {
        Some(name) if !name.is_empty() => format!("[{name}]: {effective_text}"),
        _ => effective_text.to_string(),
    };
    // Collect base64 images from attached files for Claude vision.
    let images: Vec<crate::memory::ImageData> = msg
        .files
        .iter()
        .filter_map(|f| {
            let base64 = f.base64_content.as_ref()?.clone();
            let media_type = f.mimetype.as_deref()?.to_string();
            Some(crate::memory::ImageData { base64, media_type })
        })
        .collect();

    history.push(ConversationMessage {
        role: "user".to_string(),
        content: build_message_content(&content_text, &msg.files, &msg.attachments),
        ts: Some(msg.ts.clone()),
        images,
    });

    // 3c. Set "is thinking…" typing status (best-effort, only for threaded messages).
    if !ctx.config.no_typing_channels.contains(&msg.channel) {
        if let Some(ref ts) = effective_thread_ts {
            let _ = publish_set_status(
                &ctx.js,
                &SlackSetStatusRequest {
                    channel_id: msg.channel.clone(),
                    thread_ts: ts.clone(),
                    status: Some("is thinking\u{2026}".to_string()),
                },
            )
            .await;
        }
    }

    // 4. Determine the response text (streaming or fallback).
    let response_text = if let Some(claude) = claude {
        // 4a. Open a streaming placeholder on Slack (with timeout).
        let stream_ref = match tokio::time::timeout(
            STREAM_START_TIMEOUT,
            request_stream_start(&ctx.nats, &msg.channel, effective_thread_ts.as_deref()),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::warn!("stream.start timed out after {STREAM_START_TIMEOUT:?}");
                Err("stream.start timed out".to_string())
            }
        };

        // Acquire concurrency permit when configured.
        let _permit = if let Some(ref sem) = ctx.claude_semaphore {
            Some(sem.acquire().await.expect("semaphore closed"))
        } else {
            None
        };

        // Retry loop with exponential backoff for transient Claude errors.
        let max_attempts = (ctx.config.claude_retry_attempts + 1).max(1); // at least 1 attempt
        let mut _last_error = String::new();
        let mut attempt = 0u32;
        let call_result = loop {
            attempt += 1;
            match claude.stream_response(history.clone()).await {
                Ok(result) => break Ok(result),
                Err(e) => {
                    _last_error = e.to_string();
                    if attempt >= max_attempts {
                        tracing::error!(
                            error = %e,
                            attempt,
                            "Claude API call failed after all retries"
                        );
                        break Err(e);
                    }
                    let backoff_ms = 500u64 * (1u64 << (attempt - 1).min(4)); // 500ms, 1s, 2s, 4s, 8s
                    tracing::warn!(
                        error = %e,
                        attempt,
                        backoff_ms,
                        "Claude API call failed, retrying..."
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }
            }
        };

        match stream_ref {
            Ok(stream_start) => {
                // Publish suggested prompts for fresh sessions (non-DM only).
                if !ctx.config.suggested_prompts.is_empty()
                    && history_was_empty
                    && !matches!(msg.session_type, SessionType::Direct)
                {
                    let req = SlackSetSuggestedPromptsRequest {
                        channel_id: msg.channel.clone(),
                        thread_ts: effective_thread_ts
                            .clone()
                            .unwrap_or_else(|| msg.ts.clone()),
                        title: None,
                        prompts: ctx.config.suggested_prompts.clone(),
                    };
                    if let Err(e) = publish_set_suggested_prompts(&ctx.js, &req).await {
                        tracing::warn!(error = %e, "Failed to publish suggested prompts");
                    }
                }

                // 4b. Stream Claude response; publish stream.append periodically.
                match call_result {
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
                                let delta = accumulated[last_published_len..].to_string();
                                let append = SlackStreamAppendMessage {
                                    channel: stream_start.channel.clone(),
                                    ts: stream_start.ts.clone(),
                                    text: delta,
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

                        // 4c. If upload threshold exceeded, upload as file and replace
                        //     the streamed message with a short notice.
                        let stop_text =
                            maybe_upload_response(&ctx.js, &final_text, ctx.config.upload_threshold_chars, &msg.channel, effective_thread_ts.as_deref()).await;

                        // Finalize with stream.stop.
                        let stop = SlackStreamStopMessage {
                            channel: stream_start.channel.clone(),
                            ts: stream_start.ts.clone(),
                            final_text: stop_text,
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
                    effective_thread_ts.as_deref(),
                    ctx.config.upload_threshold_chars,
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
            thread_ts: effective_thread_ts.clone(),
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

    // 4d. Clear typing status (best-effort).
    if !ctx.config.no_typing_channels.contains(&msg.channel) {
        if let Some(ref ts) = effective_thread_ts {
            let _ = publish_set_status(
                &ctx.js,
                &SlackSetStatusRequest {
                    channel_id: msg.channel.clone(),
                    thread_ts: ts.clone(),
                    status: None,
                },
            )
            .await;
        }
    }

    // 5. Persist updated history (only if we got a real response).
    if !response_text.is_empty() {
        let mut updated = history;
        updated.push(ConversationMessage {
            role: "assistant".to_string(),
            content: response_text.clone(),
            ts: None,
            images: vec![],
        });
        ctx.memory.save(&session_key, &updated).await;
    }

    // 6. Remove ack reaction.
    if let Some(emoji) = &ctx.config.ack_reaction {
        ack_reaction(&ctx.js, &msg.channel, &msg.ts, emoji, false).await;
    }
}

/// Handles a regular inbound message.
///
/// Performs early checks (DM policy, channel allowlist, user rate limit), then
/// either debounces or calls `process_inbound` directly.
pub async fn handle_inbound(msg: SlackInboundMessage, ctx: Arc<AgentContext>) {
    // DM policy check — silently ignore direct messages when disabled.
    if ctx.config.dm_policy == DmPolicy::Disabled
        && matches!(msg.session_type, SessionType::Direct)
    {
        tracing::debug!(user = %msg.user, "DM disabled by policy, dropping message");
        return;
    }

    // Channel allowlist check (DMs bypass this).
    if !ctx.config.channel_allowlist.is_empty()
        && !matches!(msg.session_type, SessionType::Direct)
        && !ctx.config.channel_allowlist.contains(&msg.channel)
    {
        tracing::debug!(channel = %msg.channel, "Channel not in allowlist, dropping message");
        return;
    }

    // User blocklist — silently drop blocked users.
    if ctx.config.user_blocklist.iter().any(|id| id == &msg.user) {
        tracing::debug!(user = %msg.user, "User is blocklisted, dropping message");
        return;
    }
    // User allowlist — only process listed users when the list is non-empty.
    if !ctx.config.user_allowlist.is_empty()
        && !ctx.config.user_allowlist.iter().any(|id| id == &msg.user)
    {
        tracing::debug!(user = %msg.user, "User not in allowlist, dropping message");
        return;
    }

    // Per-user rate limit check.
    if !ctx.user_rate_limiter.check_and_record(&msg.user) {
        tracing::warn!(user = %msg.user, "Rate limit exceeded, dropping message");
        return;
    }

    if ctx.debounce_ms > 0 {
        // Derive session key for debounce grouping (reuse msg.session_key if present,
        // otherwise fall back to the channel).
        let session_key = msg
            .session_key
            .clone()
            .unwrap_or_else(|| msg.channel.clone());

        // Cancel existing pending debounce for this session.
        {
            let mut map = ctx.session_debounce.lock().await;
            if let Some(handle) = map.remove(&session_key) {
                handle.abort();
            }
        }

        // Spawn debounced processing.
        let key = session_key.clone();
        let ctx2 = ctx.clone();
        let msg2 = msg;
        let ms = ctx.debounce_ms;
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            ctx2.session_debounce.lock().await.remove(&key);
            process_inbound(msg2, ctx2).await;
        });
        ctx.session_debounce
            .lock()
            .await
            .insert(session_key, handle.abort_handle());
        return;
    }

    process_inbound(msg, ctx).await;
}

/// If `threshold` is set and `text` exceeds it, publishes a `SlackUploadRequest`
/// and returns a short notice string. Otherwise returns `text` unchanged.
async fn maybe_upload_response(
    js: &JsContext,
    text: &str,
    threshold: Option<usize>,
    channel: &str,
    thread_ts: Option<&str>,
) -> String {
    let Some(limit) = threshold else {
        return text.to_string();
    };
    if text.len() < limit {
        return text.to_string();
    }
    let upload = SlackUploadRequest {
        channel: channel.to_string(),
        thread_ts: thread_ts.map(str::to_string),
        filename: "response.md".to_string(),
        content: text.to_string(),
        title: None,
    };
    match publish_upload_request(js, &upload).await {
        Ok(_) => "_(Response is too long — see attached file)_".to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to publish upload request — sending inline");
            text.to_string()
        }
    }
}

/// Fallback: call Claude without streaming, send response as a regular message.
async fn fallback_claude_response(
    claude: &ClaudeClient,
    history: &[ConversationMessage],
    js: &JsContext,
    channel: &str,
    thread_ts: Option<&str>,
    upload_threshold_chars: Option<usize>,
) -> String {
    match claude.stream_response(history.to_vec()).await {
        Ok((mut rx, handle)) => {
            // Drain the channel (stream runs in background task).
            while rx.recv().await.is_some() {}
            match handle.await {
                Ok(Ok(text)) => {
                    let send_text = maybe_upload_response(js, &text, upload_threshold_chars, channel, thread_ts).await;
                    let outbound = SlackOutboundMessage {
                        channel: channel.to_string(),
                        text: send_text,
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
    // `/reset` is an alias for `/clear` — both wipe the conversation history.
    if text.eq_ignore_ascii_case("clear") || text.eq_ignore_ascii_case("reset") {
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
    } else if text.eq_ignore_ascii_case("new") {
        // Clear both possible session keys for this user/channel.
        let channel_key = format!("slack:channel:{}", ev.channel_id);
        let dm_key = format!("slack:dm:{}", ev.user_id);
        ctx.memory.clear(&channel_key).await;
        ctx.memory.clear(&dm_key).await;
        {
            let mut locks = ctx.session_locks.lock().unwrap();
            locks.remove(&channel_key);
            locks.remove(&dm_key);
        }
        let greeting = "History cleared. Ready for a new conversation! How can I help you?".to_string();
        post_response_url(&ctx.http_client, &ev.response_url, &greeting).await;
        return;
    }

    // /help: post an ephemeral usage message visible only to the invoking user.
    if text.eq_ignore_ascii_case("help") {
        let help_text = format!(
            "Available commands:\n• `{cmd} clear` / `{cmd} reset` — clear conversation history\n• `{cmd} new` — start a new conversation\n• `{cmd} help` — show this help\n• `{cmd} <question>` — ask the assistant anything",
            cmd = ev.command,
        );
        let ephemeral = SlackEphemeralMessage {
            channel: ev.channel_id.clone(),
            user: ev.user_id.clone(),
            text: help_text,
            thread_ts: None,
            blocks: None,
        };
        if let Err(e) = publish_ephemeral_message(&ctx.js, &ephemeral).await {
            tracing::warn!(error = %e, "Failed to publish ephemeral help message");
        }
        return;
    }

    let prompt = format!("{} {}", ev.command, text).trim().to_string();

    let response_text = if let Some(claude) = &ctx.claude {
        let messages = vec![ConversationMessage {
            role: "user".to_string(),
            content: prompt,
            ts: None,
            images: vec![],
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

pub async fn handle_reaction(ev: SlackReactionEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        reaction = %ev.reaction,
        user = %ev.user,
        added = %ev.added,
        channel = ?ev.channel,
        item_ts = ?ev.item_ts,
        "Received reaction event"
    );
    match ev.reaction.as_str() {
        // Regenerate the last bot response.
        "arrows_counterclockwise" if ev.added => {
            let Some(channel) = ev.channel.as_deref() else { return };
            let session_key =
                derive_session_key_for_event(channel, Some(ev.user.as_str()), None)
                    .unwrap_or_else(|| format!("slack:channel:{}", channel));

            let mut history = ctx.memory.load(&session_key).await;
            // Remove the last assistant turn so it gets re-generated cleanly.
            while history.last().map(|m| m.role.as_str()) == Some("assistant") {
                history.pop();
            }
            let last_user_text = match history.last() {
                Some(m) if m.role == "user" => m.content.clone(),
                _ => {
                    tracing::debug!(
                        session_key = %session_key,
                        "Regenerate: no user message in history"
                    );
                    return;
                }
            };
            // Remove the user turn too — handle_inbound will re-add it.
            history.pop();
            ctx.memory.save(&session_key, &history).await;

            let session_type = if channel.starts_with('D') {
                SessionType::Direct
            } else if channel.starts_with('G') {
                SessionType::Group
            } else {
                SessionType::Channel
            };

            // If the previous response was uploaded as a file, delete it before regenerating.
            let cache_key = match ev.item_ts.as_deref() {
                Some(ts) => format!("{}:{}", channel, ts),
                None => channel.to_string(),
            };
            if let Some(file_id) = ctx.file_id_cache.lock().await.remove(&cache_key) {
                let del = SlackDeleteFile { file_id };
                if let Err(e) = publish_delete_file(&ctx.js, &del).await {
                    tracing::warn!(error = %e, "Failed to publish delete_file on regenerate");
                }
            }

            let inbound = SlackInboundMessage {
                channel: channel.to_string(),
                user: ev.user.clone(),
                text: last_user_text,
                ts: ev.event_ts.clone(),
                event_ts: None,
                thread_ts: None,
                parent_user_id: None,
                session_type,
                source: Some("regenerate".to_string()),
                session_key: Some(session_key),
                files: vec![],
                attachments: vec![],
                display_name: None,
            };
            handle_inbound(inbound, ctx).await;
        }

        // Remove the reacted-to message (and its paired response) from history.
        "wastebasket" if ev.added => {
            let Some(channel) = ev.channel.as_deref() else { return };
            let session_key =
                derive_session_key_for_event(channel, Some(ev.user.as_str()), None)
                    .unwrap_or_else(|| format!("slack:channel:{}", channel));

            let mut history = ctx.memory.load(&session_key).await;
            let item_ts = ev.item_ts.as_deref();

            // Try to find the matching user turn by ts.
            let idx = item_ts
                .and_then(|ts| history.iter().position(|m| m.ts.as_deref() == Some(ts)));

            if let Some(idx) = idx {
                // Remove the user turn and the assistant turn that immediately follows.
                if idx + 1 < history.len() && history[idx + 1].role == "assistant" {
                    history.remove(idx + 1);
                }
                history.remove(idx);
                tracing::info!(
                    session_key = %session_key,
                    ts = ?item_ts,
                    "Removed message pair from history"
                );
            } else {
                // No exact match — remove the most recent pair.
                if history.last().map(|m| m.role.as_str()) == Some("assistant") {
                    history.pop();
                }
                if history.last().map(|m| m.role.as_str()) == Some("user") {
                    history.pop();
                }
                tracing::info!(
                    session_key = %session_key,
                    "Removed last message pair from history (no ts match)"
                );
            }
            ctx.memory.save(&session_key, &history).await;
        }

        // Positive feedback.
        "thumbsup" | "+1" if ev.added => {
            tracing::info!(
                user = %ev.user,
                ts = ?ev.item_ts,
                "Positive feedback received"
            );
            // Only ack reactions on the bot's own messages when bot_user_id is configured.
            let on_bot_message = ctx.config.bot_user_id.as_deref()
                .map(|bot_id| ev.item_user.as_deref() == Some(bot_id))
                .unwrap_or(true); // if not configured, ack any message

            if ctx.config.reaction_notifications && on_bot_message {
                if let (Some(channel), Some(ts)) = (&ev.channel, &ev.item_ts) {
                    let ack = SlackOutboundMessage {
                        channel: channel.clone(),
                        text: ":thumbsup: Thanks for the positive feedback!".to_string(),
                        thread_ts: Some(ts.clone()),
                        blocks: None,
                        media_url: None,
                        username: None,
                        icon_url: None,
                    };
                    let _ = publish_outbound(&ctx.js, &ack).await;
                }
            }
        }

        // Negative feedback.
        "thumbsdown" | "-1" if ev.added => {
            tracing::info!(
                user = %ev.user,
                ts = ?ev.item_ts,
                "Negative feedback received"
            );
            // Only ack reactions on the bot's own messages when bot_user_id is configured.
            let on_bot_message = ctx.config.bot_user_id.as_deref()
                .map(|bot_id| ev.item_user.as_deref() == Some(bot_id))
                .unwrap_or(true); // if not configured, ack any message

            if ctx.config.reaction_notifications && on_bot_message {
                if let (Some(channel), Some(ts)) = (&ev.channel, &ev.item_ts) {
                    let ack = SlackOutboundMessage {
                        channel: channel.clone(),
                        text: ":thumbsdown: Thanks for the feedback — I'll try to do better!".to_string(),
                        thread_ts: Some(ts.clone()),
                        blocks: None,
                        media_url: None,
                        username: None,
                        icon_url: None,
                    };
                    let _ = publish_outbound(&ctx.js, &ack).await;
                }
            }
        }

        _ => {}
    }
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
        display_name: None,
    };
    handle_inbound(inbound, ctx).await;
}

pub async fn handle_block_action(ev: SlackBlockActionEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        action_id = %ev.action_id,
        user_id = %ev.user_id,
        channel_id = ?ev.channel_id,
        message_ts = ?ev.message_ts,
        "Received block action"
    );
    match ev.action_id.as_str() {
        "clear_history" => {
            let session_key = match ev.channel_id.as_deref() {
                Some(ch) if ch.starts_with('D') => format!("slack:dm:{}", ev.user_id),
                Some(ch) if ch.starts_with('G') => format!("slack:group:{}", ch),
                Some(ch) => format!("slack:channel:{}", ch),
                None => format!("slack:dm:{}", ev.user_id), // triggered from App Home
            };
            ctx.memory.clear(&session_key).await;
            ctx.session_locks.lock().unwrap().remove(&session_key);
            tracing::info!(
                session_key = %session_key,
                user = %ev.user_id,
                "History cleared via block action"
            );
            if ev.channel_id.is_none() {
                refresh_app_home(&ctx, &ev.user_id).await;
            }
        }
        "open_settings" => {
            let trigger_id = match ev.trigger_id {
                Some(t) => t,
                None => {
                    tracing::warn!(user = %ev.user_id, "open_settings without trigger_id");
                    return;
                }
            };
            let settings = ctx.user_settings.load(&ev.user_id).await;
            let view = build_settings_modal(&settings, &ctx.config.claude_model);
            let req = SlackViewOpenRequest { trigger_id, view };
            if let Err(e) = publish_view_open(&ctx.js, &req).await {
                tracing::error!(error = %e, "Failed to publish views.open for settings");
            }
        }
        "feedback_positive" | "feedback_negative" => {
            let positive = ev.action_id == "feedback_positive";
            tracing::info!(
                user = %ev.user_id,
                message_ts = ?ev.message_ts,
                positive,
                "Response feedback received"
            );
            // Future: persist feedback to a dedicated NATS KV bucket.
        }
        _ => {
            tracing::debug!(action_id = %ev.action_id, "Unhandled block action");
        }
    }
}

pub async fn handle_member(ev: SlackMemberEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        user = %ev.user,
        channel = %ev.channel,
        joined = %ev.joined,
        "Received member event"
    );
    if ev.joined
        && let Some(ref msg) = ctx.config.welcome_message
    {
        let outbound = SlackOutboundMessage {
            channel: ev.channel.clone(),
            text: msg.clone(),
            thread_ts: None,
            blocks: None,
            media_url: None,
            username: None,
            icon_url: None,
        };
        if let Err(e) = publish_outbound(&ctx.js, &outbound).await {
            tracing::warn!(
                error = %e,
                channel = %ev.channel,
                "Failed to publish welcome message"
            );
        }
    }
}

pub async fn handle_channel(ev: SlackChannelEvent) {
    tracing::info!(
        channel_id = %ev.channel_id,
        channel_name = ?ev.channel_name,
        kind = ?ev.kind,
        "Received channel event"
    );
}

pub async fn handle_app_home(ev: SlackAppHomeOpenedEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        user = %ev.user,
        tab = %ev.tab,
        "Received app_home_opened event"
    );
    // Only render the Home tab, not the Messages tab.
    if ev.tab == "messages" {
        return;
    }
    refresh_app_home(&ctx, &ev.user).await;
}

pub async fn handle_view_submission(ev: SlackViewSubmissionEvent, ctx: Arc<AgentContext>) {
    tracing::info!(
        user_id = %ev.user_id,
        view_id = %ev.view_id,
        callback_id = ?ev.callback_id,
        "Received view_submission event"
    );
    match ev.callback_id.as_deref() {
        Some("user_settings") => {
            let model = ev
                .values
                .pointer("/values/model_block/model_select/selected_option/value")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let system_prompt = ev
                .values
                .pointer("/values/system_prompt_block/system_prompt_input/value")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let settings = UserSettings { model, system_prompt };
            ctx.user_settings.save(&ev.user_id, &settings).await;
            tracing::info!(user = %ev.user_id, model = ?settings.model, "User settings saved");
            refresh_app_home(&ctx, &ev.user_id).await;
        }
        other => {
            tracing::debug!(callback_id = ?other, "Unhandled view submission");
        }
    }
}

pub async fn handle_view_closed(ev: SlackViewClosedEvent, _ctx: Arc<AgentContext>) {
    tracing::info!(
        user_id = %ev.user_id,
        view_id = %ev.view_id,
        callback_id = ?ev.callback_id,
        "View closed without submission"
    );
    // No action needed — just log. Future: could clean up pending state.
}

pub async fn handle_pin(ev: SlackPinEvent, ctx: Arc<AgentContext>) {
    let add = matches!(ev.kind, PinEventKind::Added);
    tracing::info!(channel = %ev.channel, item_ts = ?ev.item_ts, added = add, "Pin event");
    if let Some(ref ts) = ev.item_ts {
        let action = SlackReactionAction {
            channel: ev.channel.clone(),
            ts: ts.clone(),
            reaction: "pushpin".to_string(),
            add,
        };
        if let Err(e) = publish_reaction_action(&ctx.js, &action).await {
            tracing::warn!(error = %e, "Failed to publish pin reaction action");
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the content string sent to Claude.
///
/// For text-like files where slack-bot was able to download the content, the
/// full text is embedded between fences so Claude can read and reason about it.
/// For binary or oversized files, only metadata (name + MIME type) is included.
///
/// Non-empty attachments are appended as an `[Attachments: ...]` block. Each
/// attachment contributes its `text` field if present, or its `fallback` if
/// `text` is absent. Attachments with neither field are skipped.
fn build_message_content(text: &str, files: &[SlackFile], attachments: &[SlackAttachment]) -> String {
    let mut content = text.to_string();

    if !files.is_empty() {
        content.push_str("\n\n[Attached files:");
        for f in files {
            let name = f.name.as_deref().unwrap_or("unknown");
            let mime = f.mimetype.as_deref().unwrap_or("unknown type");
            if let Some(ref file_text) = f.content {
                content.push_str(&format!(
                    "\n- {} ({}):\n```\n{}\n```",
                    name, mime, file_text
                ));
            } else {
                content.push_str(&format!("\n- {} ({})", name, mime));
            }
        }
        content.push(']');
    }

    // Build the attachments block, skipping entries with no displayable text.
    let attachment_lines: Vec<&str> = attachments
        .iter()
        .filter_map(|a| {
            a.text
                .as_deref()
                .or_else(|| a.fallback.as_deref())
                .filter(|s| !s.is_empty())
        })
        .collect();

    if !attachment_lines.is_empty() {
        content.push_str("\n\n[Attachments:");
        for line in attachment_lines {
            content.push_str(&format!("\n- {}", line));
        }
        content.push(']');
    }

    content
}

/// Parse manual reply-targeting directives from the message text.
///
/// Directives are stripped from the returned text:
/// - `[[reply_to_current]]`    — thread the reply under the message's own `ts`
/// - `[[reply_to:<ts>]]`       — thread the reply under a specific `ts`
fn extract_reply_target(text: &str, current_ts: &str) -> (String, Option<String>) {
    if let Some(idx) = text.find("[[reply_to_current]]") {
        let cleaned = (text[..idx].to_string() + &text[idx + "[[reply_to_current]]".len()..])
            .trim()
            .to_string();
        return (cleaned, Some(current_ts.to_string()));
    }
    if let Some(start) = text.find("[[reply_to:") {
        let after = &text[start + "[[reply_to:".len()..];
        if let Some(end) = after.find("]]") {
            let ts = after[..end].to_string();
            let full_tag = format!("[[reply_to:{ts}]]");
            let cleaned = text.replacen(&full_tag, "", 1).trim().to_string();
            return (cleaned, Some(ts));
        }
    }
    (text.to_string(), None)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReplyToMode;

    // ── UserRateLimiter ───────────────────────────────────────────────────────

    #[test]
    fn rate_limiter_disabled_when_zero() {
        let rl = UserRateLimiter::new(0);
        for _ in 0..100 {
            assert!(rl.check_and_record("U1"), "limit=0 should always allow");
        }
    }

    #[test]
    fn rate_limiter_allows_within_limit() {
        let rl = UserRateLimiter::new(3);
        assert!(rl.check_and_record("U1"));
        assert!(rl.check_and_record("U1"));
        assert!(rl.check_and_record("U1"));
    }

    #[test]
    fn rate_limiter_blocks_when_over_limit() {
        let rl = UserRateLimiter::new(3);
        assert!(rl.check_and_record("U1"));
        assert!(rl.check_and_record("U1"));
        assert!(rl.check_and_record("U1"));
        assert!(!rl.check_and_record("U1"), "4th call should be blocked");
    }

    #[test]
    fn rate_limiter_different_users_independent() {
        let rl = UserRateLimiter::new(2);
        assert!(rl.check_and_record("A"));
        assert!(rl.check_and_record("A"));
        // A is now at limit.
        assert!(!rl.check_and_record("A"), "A should be blocked");
        // B should still be allowed.
        assert!(rl.check_and_record("B"), "B should be unaffected by A's limit");
        assert!(rl.check_and_record("B"), "B's 2nd call should be allowed");
        assert!(!rl.check_and_record("B"), "B's 3rd call should be blocked");
    }

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

    fn make_attachment(text: Option<&str>, fallback: Option<&str>) -> SlackAttachment {
        SlackAttachment {
            text: text.map(str::to_string),
            fallback: fallback.map(str::to_string),
            pretext: None,
            author_name: None,
            from_url: None,
            image_url: None,
            thumb_url: None,
            channel_id: None,
            channel_name: None,
            ts: None,
            files: vec![],
        }
    }

    #[test]
    fn no_files_returns_text_unchanged() {
        assert_eq!(build_message_content("hello", &[], &[]), "hello");
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
            content: None,
            base64_content: None,
        }];
        let result = build_message_content("look at this", &files, &[]);
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
            content: None,
            base64_content: None,
        }];
        let result = build_message_content("hi", &files, &[]);
        assert!(result.contains("unknown"));
        assert!(result.contains("unknown type"));
    }

    #[test]
    fn attachments_with_text_are_included() {
        let attachments = vec![make_attachment(Some("This is the attachment text"), None)];
        let result = build_message_content("msg", &[], &attachments);
        assert!(result.contains("[Attachments:"));
        assert!(result.contains("- This is the attachment text"));
    }

    #[test]
    fn attachments_with_fallback_only_are_included() {
        let attachments = vec![make_attachment(None, Some("fallback text here"))];
        let result = build_message_content("msg", &[], &attachments);
        assert!(result.contains("[Attachments:"));
        assert!(result.contains("- fallback text here"));
    }

    #[test]
    fn attachments_with_text_prefer_text_over_fallback() {
        let attachments = vec![make_attachment(
            Some("primary text"),
            Some("fallback text"),
        )];
        let result = build_message_content("msg", &[], &attachments);
        assert!(result.contains("- primary text"));
        assert!(!result.contains("fallback text"));
    }

    #[test]
    fn attachments_with_neither_text_nor_fallback_are_skipped() {
        let attachments = vec![make_attachment(None, None)];
        let result = build_message_content("msg", &[], &attachments);
        assert!(!result.contains("[Attachments:"));
        assert_eq!(result, "msg");
    }

    #[test]
    fn empty_attachments_vec_appends_no_block() {
        let result = build_message_content("hello", &[], &[]);
        assert!(!result.contains("[Attachments:"));
        assert_eq!(result, "hello");
    }

    #[test]
    fn mix_of_files_and_attachments() {
        let files = vec![SlackFile {
            id: Some("F2".into()),
            name: Some("doc.txt".into()),
            mimetype: Some("text/plain".into()),
            url_private: None,
            url_private_download: None,
            size: None,
            content: Some("file contents".into()),
            base64_content: None,
        }];
        let attachments = vec![make_attachment(Some("attachment body"), None)];
        let result = build_message_content("base text", &files, &attachments);
        assert!(result.starts_with("base text"));
        assert!(result.contains("[Attached files:"));
        assert!(result.contains("doc.txt"));
        assert!(result.contains("file contents"));
        assert!(result.contains("[Attachments:"));
        assert!(result.contains("- attachment body"));
        // Files block must come before Attachments block.
        let files_pos = result.find("[Attached files:").unwrap();
        let attachments_pos = result.find("[Attachments:").unwrap();
        assert!(files_pos < attachments_pos);
    }

    // ── extract_reply_target ──────────────────────────────────────────────────

    #[test]
    fn reply_target_current_directive() {
        let (text, ts) = extract_reply_target("hello [[reply_to_current]]", "1234.0");
        assert_eq!(text, "hello");
        assert_eq!(ts, Some("1234.0".to_string()));
    }

    #[test]
    fn reply_target_specific_ts() {
        let (text, ts) = extract_reply_target("hi [[reply_to:9999.1]]", "1234.0");
        assert_eq!(text, "hi");
        assert_eq!(ts, Some("9999.1".to_string()));
    }

    #[test]
    fn reply_target_none_when_no_directive() {
        let (text, ts) = extract_reply_target("hello world", "1234.0");
        assert_eq!(text, "hello world");
        assert_eq!(ts, None);
    }
}
