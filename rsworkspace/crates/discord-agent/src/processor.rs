//! Message processor - handles business logic for different Discord event types

use anyhow::Result;
use discord_nats::{subjects, MessagePublisher};
use discord_types::{
    events::{
        AutocompleteEvent, BotReadyEvent, ChannelCreateEvent, ChannelDeleteEvent,
        ChannelUpdateEvent, ComponentInteractionEvent, GuildCreateEvent, GuildDeleteEvent,
        GuildMemberAddEvent, GuildMemberRemoveEvent, GuildMemberUpdateEvent, GuildUpdateEvent,
        MessageCreatedEvent, MessageDeletedEvent, MessageUpdatedEvent, ModalSubmitEvent,
        PresenceUpdateEvent, ReactionAddEvent, ReactionRemoveEvent, RoleCreateEvent,
        RoleDeleteEvent, RoleUpdateEvent, SlashCommandEvent, TypingStartEvent, VoiceStateUpdateEvent,
    },
    types::{Attachment, ChannelType},
    AutocompleteChoice, AutocompleteRespondCommand, InteractionDeferCommand,
    InteractionFollowupCommand, InteractionRespondCommand, SendMessageCommand, StreamMessageCommand,
    TypingCommand,
};
use tokio::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::conversation::ConversationManager;
use crate::health::AgentMetrics;
use crate::llm::{ClaudeClient, ClaudeConfig};

/// Minimum interval between stream-update publishes to avoid flooding NATS.
const STREAM_PUBLISH_INTERVAL: Duration = Duration::from_millis(500);

/// Cursor appended to in-progress streaming messages.
const STREAM_CURSOR: &str = "▌";

/// Default system prompt used when none is configured.
const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful AI assistant in a Discord server. \
    Be concise, friendly, and helpful. Keep responses under 500 words unless \
    the user specifically asks for more detail.";

/// Default welcome message template.
pub const DEFAULT_WELCOME_TEMPLATE: &str = "Welcome to the server, {user}! 👋";

/// Default farewell message template.
pub const DEFAULT_FAREWELL_TEMPLATE: &str = "**{username}** has left the server.";

/// Configuration for welcome or farewell messages sent when members join/leave.
pub struct WelcomeConfig {
    /// Channel to send the message in.
    pub channel_id: u64,
    /// Message template. Supports `{user}` (mention) and `{username}` (display name).
    pub template: String,
}

/// Message processor
pub struct MessageProcessor {
    llm_client: Option<ClaudeClient>,
    pub(crate) conversation_manager: ConversationManager,
    system_prompt: String,
    welcome: Option<WelcomeConfig>,
    farewell: Option<WelcomeConfig>,
    metrics: Option<AgentMetrics>,
    /// Maximum wall-clock time allowed for a single LLM streaming response.
    stream_timeout: Duration,
}

impl MessageProcessor {
    /// Create a new message processor
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm_config: Option<ClaudeConfig>,
        conversation_kv: Option<async_nats::jetstream::kv::Store>,
        system_prompt: Option<String>,
        welcome: Option<WelcomeConfig>,
        farewell: Option<WelcomeConfig>,
        conversation_ttl: Option<Duration>,
        metrics: Option<AgentMetrics>,
        max_history: usize,
        stream_timeout: Duration,
    ) -> Self {
        let conversation_manager = match conversation_kv {
            Some(kv) => ConversationManager::with_kv_and_max_history(kv, max_history),
            None => ConversationManager::with_max_history(max_history),
        };
        if let Some(ttl) = conversation_ttl {
            conversation_manager.start_cleanup(ttl);
        }
        Self {
            llm_client: llm_config.map(ClaudeClient::new),
            conversation_manager,
            system_prompt: system_prompt.unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
            welcome,
            farewell,
            metrics,
            stream_timeout,
        }
    }

    /// Process a regular channel message
    pub async fn process_message(
        &self,
        event: &MessageCreatedEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let channel_id = event.message.channel_id;
        let message_id = event.message.id;
        let session_id = &event.metadata.session_id;
        let content = &event.message.content;

        // Skip empty messages
        if content.is_empty() {
            return Ok(());
        }

        // Send typing indicator
        self.send_typing(channel_id, publisher).await?;

        if let Some(ref llm_client) = self.llm_client {
            let history = self.conversation_manager.get_history(session_id).await;

            let system_prompt = self.system_prompt.clone();

            // Record the Discord message ID so edit/delete events can find this
            // exact turn by ID rather than falling back to positional heuristics.
            self.conversation_manager
                .add_user_message(session_id, content, message_id)
                .await;

            // Stream the response progressively
            let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<String>(64);
            let stream_subject = subjects::agent::message_stream(publisher.prefix());

            // Send placeholder to create the message immediately
            publisher
                .publish(
                    &stream_subject,
                    &StreamMessageCommand {
                        channel_id,
                        content: STREAM_CURSOR.to_string(),
                        is_final: false,
                        session_id: session_id.clone(),
                        reply_to_message_id: Some(message_id),
                    },
                )
                .await?;

            // Clone owned data for the spawned task (tokio::spawn requires 'static)
            let llm_client = llm_client.clone();
            let history = history.clone();
            // Prepend reply context so the LLM knows what the user is responding to
            let reply_ctx =
                Self::format_reply_context(event.message.referenced_message_content.as_deref());
            // Append attachment metadata so the LLM is aware of any uploaded files
            let attachment_ctx = Self::format_attachments(&event.message.attachments);
            let content_owned = format!("{}{}{}", reply_ctx, content, attachment_ctx);

            let llm_handle = tokio::spawn(async move {
                llm_client
                    .generate_response_streaming(&system_prompt, &content_owned, &history, chunk_tx)
                    .await
            });

            let mut accumulated = String::new();
            let mut last_publish = Instant::now();

            // Consume chunks and publish throttled updates (with overall timeout)
            let stream_deadline = tokio::time::sleep(self.stream_timeout);
            tokio::pin!(stream_deadline);
            let mut timed_out = false;

            loop {
                tokio::select! {
                    chunk = chunk_rx.recv() => {
                        match chunk {
                            Some(c) => {
                                accumulated.push_str(&c);
                                if last_publish.elapsed() >= STREAM_PUBLISH_INTERVAL {
                                    publisher
                                        .publish(
                                            &stream_subject,
                                            &StreamMessageCommand {
                                                channel_id,
                                                content: format!("{}{}", accumulated, STREAM_CURSOR),
                                                is_final: false,
                                                session_id: session_id.clone(),
                                                reply_to_message_id: None,
                                            },
                                        )
                                        .await?;
                                    last_publish = Instant::now();
                                }
                            }
                            None => break,
                        }
                    }
                    _ = &mut stream_deadline => {
                        timed_out = true;
                        llm_handle.abort();
                        break;
                    }
                }
            }

            if timed_out {
                warn!(
                    "LLM response timed out after {:?} for session {}",
                    self.stream_timeout, session_id
                );
                if let Some(ref m) = self.metrics {
                    m.inc_llm_errors();
                }
                publisher
                    .publish(
                        &stream_subject,
                        &StreamMessageCommand {
                            channel_id,
                            content: "Sorry, the response timed out. Please try again.".to_string(),
                            is_final: true,
                            session_id: session_id.clone(),
                            reply_to_message_id: None,
                        },
                    )
                    .await?;
                return Ok(());
            }

            match llm_handle.await? {
                Ok(full_response) => {
                    self.conversation_manager
                        .add_message(session_id, "assistant", &full_response)
                        .await;

                    publisher
                        .publish(
                            &stream_subject,
                            &StreamMessageCommand {
                                channel_id,
                                content: full_response,
                                is_final: true,
                                session_id: session_id.clone(),
                                reply_to_message_id: None,
                            },
                        )
                        .await?;

                    if let Some(ref m) = self.metrics {
                        m.inc_messages_processed();
                    }
                    debug!("Streamed LLM response to channel {}", channel_id);
                }
                Err(e) => {
                    if let Some(ref m) = self.metrics {
                        m.inc_llm_errors();
                    }
                    warn!("LLM streaming failed for session {}: {}", session_id, e);
                    // Replace the placeholder with an error message
                    publisher
                        .publish(
                            &stream_subject,
                            &StreamMessageCommand {
                                channel_id,
                                content: "Sorry, I encountered an error generating a response. \
                                    Please try again."
                                    .to_string(),
                                is_final: true,
                                session_id: session_id.clone(),
                                reply_to_message_id: None,
                            },
                        )
                        .await?;
                }
            }
        } else {
            // Echo mode (no LLM)
            let cmd = SendMessageCommand {
                channel_id,
                content: format!("You said: {}", content),
                embeds: vec![],
                reply_to_message_id: Some(message_id),
                files: vec![],
                components: vec![],
            };
            let subject = subjects::agent::message_send(publisher.prefix());
            publisher.publish(&subject, &cmd).await?;
        }

        Ok(())
    }

    /// Process a slash command interaction
    pub async fn process_slash_command(
        &self,
        event: &SlashCommandEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let session_id = &event.metadata.session_id;

        match event.command_name.as_str() {
            "ping" => {
                self.interaction_respond(
                    event.interaction_id,
                    &event.interaction_token,
                    "Pong! 🏓",
                    false,
                    publisher,
                )
                .await?;
            }

            "help" => {
                let text = if self.llm_client.is_some() {
                    "**Discord AI Agent**\n\n\
                    Available commands:\n\
                    `/ping` — Check if the bot is alive\n\
                    `/help` — Show this message\n\
                    `/status` — Show agent status\n\
                    `/clear` — Clear your conversation history\n\
                    `/forget` — Remove the last message exchange from memory\n\
                    `/summarize` — Summarize your conversation so far\n\
                    `/ask <question>` — Ask the AI a question\n\n\
                    You can also just send a message in any allowed channel!\n\
                    React with 🔁 to regenerate a response or ❌ to clear history."
                } else {
                    "**Discord AI Agent** *(echo mode)*\n\n\
                    Available commands:\n\
                    `/ping` — Check if the bot is alive\n\
                    `/help` — Show this message\n\
                    `/status` — Show agent status\n\
                    `/clear` — Clear your conversation history\n\
                    `/forget` — Remove the last message exchange from memory\n\n\
                    You can also just send a message in any allowed channel!"
                };
                self.interaction_respond(
                    event.interaction_id,
                    &event.interaction_token,
                    text,
                    false,
                    publisher,
                )
                .await?;
            }

            "status" => {
                let active = self.conversation_manager.active_sessions().await;
                let mode = if self.llm_client.is_some() {
                    "LLM (Claude AI)"
                } else {
                    "Echo mode"
                };
                let text = if let Some(ref m) = self.metrics {
                    format!(
                        "**Agent Status**\nMode: {}\nActive sessions: {}\n\
                        Messages processed: {}\nLLM errors: {}\nReady ✅",
                        mode,
                        active,
                        m.messages_processed(),
                        m.llm_errors(),
                    )
                } else {
                    format!(
                        "**Agent Status**\nMode: {}\nActive sessions: {}\nReady ✅",
                        mode, active
                    )
                };
                self.interaction_respond(
                    event.interaction_id,
                    &event.interaction_token,
                    &text,
                    false,
                    publisher,
                )
                .await?;
            }

            "summarize" => {
                let history = self.conversation_manager.get_history(session_id).await;

                if history.is_empty() {
                    self.interaction_respond(
                        event.interaction_id,
                        &event.interaction_token,
                        "No conversation history to summarize.",
                        true,
                        publisher,
                    )
                    .await?;
                } else if let Some(ref llm_client) = self.llm_client {
                    self.interaction_defer(
                        event.interaction_id,
                        &event.interaction_token,
                        false,
                        publisher,
                    )
                    .await?;

                    // Build a plain-text transcript for the LLM to summarize
                    let transcript: String = history
                        .iter()
                        .map(|m| format!("{}: {}", m.role, m.content))
                        .collect::<Vec<_>>()
                        .join("\n");

                    let summarize_prompt = format!(
                        "Please provide a concise summary of this conversation:\n\n{}",
                        transcript
                    );

                    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<String>(64);
                    let llm_client = llm_client.clone();
                    let system_prompt = self.system_prompt.clone();

                    let llm_handle = tokio::spawn(async move {
                        llm_client
                            .generate_response_streaming(
                                &system_prompt,
                                &summarize_prompt,
                                &[],
                                chunk_tx,
                            )
                            .await
                    });

                    let ask_session = format!("summarize_{}", session_id);
                    let followup_subject =
                        subjects::agent::interaction_followup(publisher.prefix());
                    publisher
                        .publish(
                            &followup_subject,
                            &InteractionFollowupCommand {
                                interaction_token: event.interaction_token.clone(),
                                content: Some(STREAM_CURSOR.to_string()),
                                embeds: vec![],
                                ephemeral: false,
                                session_id: Some(ask_session.clone()),
                                files: vec![],
                                components: vec![],
                            },
                        )
                        .await?;

                    let stream_subject = subjects::agent::message_stream(publisher.prefix());
                    let mut accumulated = String::new();
                    let mut last_publish = Instant::now();

                    let stream_deadline = tokio::time::sleep(self.stream_timeout);
                    tokio::pin!(stream_deadline);
                    let mut timed_out = false;

                    loop {
                        tokio::select! {
                            chunk = chunk_rx.recv() => {
                                match chunk {
                                    Some(c) => {
                                        accumulated.push_str(&c);
                                        if last_publish.elapsed() >= STREAM_PUBLISH_INTERVAL {
                                            publisher
                                                .publish(
                                                    &stream_subject,
                                                    &StreamMessageCommand {
                                                        channel_id: event.channel_id,
                                                        content: format!("{}{}", accumulated, STREAM_CURSOR),
                                                        is_final: false,
                                                        session_id: ask_session.clone(),
                                                        reply_to_message_id: None,
                                                    },
                                                )
                                                .await?;
                                            last_publish = Instant::now();
                                        }
                                    }
                                    None => break,
                                }
                            }
                            _ = &mut stream_deadline => {
                                timed_out = true;
                                llm_handle.abort();
                                break;
                            }
                        }
                    }

                    if timed_out {
                        warn!(
                            "LLM summarize timed out after {:?} for session {}",
                            self.stream_timeout, session_id
                        );
                        if let Some(ref m) = self.metrics {
                            m.inc_llm_errors();
                        }
                        publisher
                            .publish(
                                &stream_subject,
                                &StreamMessageCommand {
                                    channel_id: event.channel_id,
                                    content: "Sorry, the summary timed out. Please try again."
                                        .to_string(),
                                    is_final: true,
                                    session_id: ask_session,
                                    reply_to_message_id: None,
                                },
                            )
                            .await?;
                        return Ok(());
                    }

                    match llm_handle.await? {
                        Ok(summary) => {
                            if let Some(ref m) = self.metrics {
                                m.inc_messages_processed();
                            }
                            publisher
                                .publish(
                                    &stream_subject,
                                    &StreamMessageCommand {
                                        channel_id: event.channel_id,
                                        content: summary,
                                        is_final: true,
                                        session_id: ask_session,
                                        reply_to_message_id: None,
                                    },
                                )
                                .await?;
                        }
                        Err(e) => {
                            if let Some(ref m) = self.metrics {
                                m.inc_llm_errors();
                            }
                            warn!("LLM summarize failed for session {}: {}", session_id, e);
                            publisher
                                .publish(
                                    &stream_subject,
                                    &StreamMessageCommand {
                                        channel_id: event.channel_id,
                                        content: "Sorry, I couldn't generate a summary. Please try again.".to_string(),
                                        is_final: true,
                                        session_id: ask_session,
                                        reply_to_message_id: None,
                                    },
                                )
                                .await?;
                        }
                    }
                } else {
                    self.interaction_respond(
                        event.interaction_id,
                        &event.interaction_token,
                        "Summarization requires LLM mode.",
                        true,
                        publisher,
                    )
                    .await?;
                }
            }

            "clear" => {
                self.conversation_manager.clear_session(session_id).await;
                self.interaction_respond(
                    event.interaction_id,
                    &event.interaction_token,
                    "Conversation history cleared!",
                    true,
                    publisher,
                )
                .await?;
            }

            "forget" => {
                let mut history = self.conversation_manager.get_history(session_id).await;

                if history.is_empty() {
                    self.interaction_respond(
                        event.interaction_id,
                        &event.interaction_token,
                        "No conversation history to forget.",
                        true,
                        publisher,
                    )
                    .await?;
                } else {
                    // Remove the last assistant response (if any), then the last user message
                    if history.last().map(|m| m.role.as_str()) == Some("assistant") {
                        history.pop();
                    }
                    if history.last().map(|m| m.role.as_str()) == Some("user") {
                        history.pop();
                    }
                    self.conversation_manager
                        .save_history(session_id, history)
                        .await;
                    self.interaction_respond(
                        event.interaction_id,
                        &event.interaction_token,
                        "Last exchange removed from memory.",
                        true,
                        publisher,
                    )
                    .await?;
                }
            }

            "ask" => {
                // Get the question from command options
                let question = event
                    .options
                    .iter()
                    .find(|o| o.name == "question")
                    .and_then(|o| {
                        if let discord_types::types::CommandOptionValue::String(s) = &o.value {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();

                if question.is_empty() {
                    self.interaction_respond(
                        event.interaction_id,
                        &event.interaction_token,
                        "Please provide a question.",
                        true,
                        publisher,
                    )
                    .await?;
                    return Ok(());
                }

                if let Some(ref llm_client) = self.llm_client {
                    // Defer first so Discord doesn't time out while we stream
                    self.interaction_defer(
                        event.interaction_id,
                        &event.interaction_token,
                        false,
                        publisher,
                    )
                    .await?;

                    let history = self.conversation_manager.get_history(session_id).await;
                    let system_prompt = self.system_prompt.clone();

                    self.conversation_manager
                        .add_message(session_id, "user", &question)
                        .await;

                    // session_id used to correlate the followup message with
                    // subsequent StreamMessageCommands
                    let ask_session = format!("ask_{}", session_id);

                    // Send initial followup with cursor; include session_id so
                    // the bot registers the resulting message_id for streaming
                    let followup_subject =
                        subjects::agent::interaction_followup(publisher.prefix());
                    publisher
                        .publish(
                            &followup_subject,
                            &InteractionFollowupCommand {
                                interaction_token: event.interaction_token.clone(),
                                content: Some(STREAM_CURSOR.to_string()),
                                embeds: vec![],
                                ephemeral: false,
                                session_id: Some(ask_session.clone()),
                                files: vec![],
                                components: vec![],
                            },
                        )
                        .await?;

                    // Clone owned data for the spawned task (tokio::spawn requires 'static)
                    let llm_client = llm_client.clone();
                    let history = history.clone();
                    let question_owned = question.clone();

                    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<String>(64);
                    let llm_handle = tokio::spawn(async move {
                        llm_client
                            .generate_response_streaming(
                                &system_prompt,
                                &question_owned,
                                &history,
                                chunk_tx,
                            )
                            .await
                    });

                    // Stream updates edit the followup message via its session_id
                    let stream_subject = subjects::agent::message_stream(publisher.prefix());
                    let mut accumulated = String::new();
                    let mut last_publish = Instant::now();

                    let stream_deadline = tokio::time::sleep(self.stream_timeout);
                    tokio::pin!(stream_deadline);
                    let mut timed_out = false;

                    loop {
                        tokio::select! {
                            chunk = chunk_rx.recv() => {
                                match chunk {
                                    Some(c) => {
                                        accumulated.push_str(&c);
                                        if last_publish.elapsed() >= STREAM_PUBLISH_INTERVAL {
                                            publisher
                                                .publish(
                                                    &stream_subject,
                                                    &StreamMessageCommand {
                                                        channel_id: event.channel_id,
                                                        content: format!("{}{}", accumulated, STREAM_CURSOR),
                                                        is_final: false,
                                                        session_id: ask_session.clone(),
                                                        reply_to_message_id: None,
                                                    },
                                                )
                                                .await?;
                                            last_publish = Instant::now();
                                        }
                                    }
                                    None => break,
                                }
                            }
                            _ = &mut stream_deadline => {
                                timed_out = true;
                                llm_handle.abort();
                                break;
                            }
                        }
                    }

                    if timed_out {
                        warn!(
                            "LLM /ask timed out after {:?} for session {}",
                            self.stream_timeout, session_id
                        );
                        if let Some(ref m) = self.metrics {
                            m.inc_llm_errors();
                        }
                        publisher
                            .publish(
                                &stream_subject,
                                &StreamMessageCommand {
                                    channel_id: event.channel_id,
                                    content: "Sorry, the response timed out. Please try again."
                                        .to_string(),
                                    is_final: true,
                                    session_id: ask_session,
                                    reply_to_message_id: None,
                                },
                            )
                            .await?;
                        return Ok(());
                    }

                    match llm_handle.await? {
                        Ok(full_response) => {
                            self.conversation_manager
                                .add_message(session_id, "assistant", &full_response)
                                .await;

                            if let Some(ref m) = self.metrics {
                                m.inc_messages_processed();
                            }

                            publisher
                                .publish(
                                    &stream_subject,
                                    &StreamMessageCommand {
                                        channel_id: event.channel_id,
                                        content: full_response,
                                        is_final: true,
                                        session_id: ask_session,
                                        reply_to_message_id: None,
                                    },
                                )
                                .await?;
                        }
                        Err(e) => {
                            if let Some(ref m) = self.metrics {
                                m.inc_llm_errors();
                            }
                            warn!(
                                "LLM streaming failed for /ask in session {}: {}",
                                session_id, e
                            );
                            publisher
                                .publish(
                                    &stream_subject,
                                    &StreamMessageCommand {
                                        channel_id: event.channel_id,
                                        content: "Sorry, I encountered an error. Please try again."
                                            .to_string(),
                                        is_final: true,
                                        session_id: ask_session,
                                        reply_to_message_id: None,
                                    },
                                )
                                .await?;
                        }
                    }
                } else {
                    // Echo mode for /ask
                    self.interaction_respond(
                        event.interaction_id,
                        &event.interaction_token,
                        &format!("You asked: {}", question),
                        false,
                        publisher,
                    )
                    .await?;
                }
            }

            other => {
                info!("Unknown slash command: /{}", other);
                self.interaction_respond(
                    event.interaction_id,
                    &event.interaction_token,
                    "Unknown command. Use `/help` to see available commands.",
                    true,
                    publisher,
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Process a guild member join event
    pub async fn process_member_add(
        &self,
        event: &GuildMemberAddEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let Some(ref cfg) = self.welcome else {
            return Ok(());
        };

        let user = &event.member.user;
        let display_name = event
            .member
            .nick
            .as_deref()
            .or(user.global_name.as_deref())
            .unwrap_or(&user.username);

        let content = cfg
            .template
            .replace("{user}", &format!("<@{}>", user.id))
            .replace("{username}", display_name);

        let cmd = SendMessageCommand {
            channel_id: cfg.channel_id,
            content,
            embeds: vec![],
            reply_to_message_id: None,
            files: vec![],
            components: vec![],
        };
        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;

        info!(
            "Sent welcome message for user {} in guild {}",
            user.username, event.guild_id
        );
        Ok(())
    }

    /// Process a guild member leave event
    pub async fn process_member_remove(
        &self,
        event: &GuildMemberRemoveEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let Some(ref cfg) = self.farewell else {
            return Ok(());
        };

        let user = &event.user;
        let display_name = user.global_name.as_deref().unwrap_or(&user.username);

        let content = cfg
            .template
            .replace("{user}", &format!("<@{}>", user.id))
            .replace("{username}", display_name);

        let cmd = SendMessageCommand {
            channel_id: cfg.channel_id,
            content,
            embeds: vec![],
            reply_to_message_id: None,
            files: vec![],
            components: vec![],
        };
        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;

        info!(
            "Sent farewell message for user {} in guild {}",
            user.username, event.guild_id
        );
        Ok(())
    }

    /// Process a message edit event
    ///
    /// Finds the turn by Discord message ID when available; falls back to
    /// updating the most recent user turn for history that pre-dates ID tracking.
    pub async fn process_message_updated(&self, event: &MessageUpdatedEvent) -> Result<()> {
        let Some(ref new_content) = event.new_content else {
            return Ok(());
        };

        let session_id = &event.metadata.session_id;
        let mut history = self.conversation_manager.get_history(session_id).await;

        // Prefer exact ID match; fall back to last user message for old history.
        // Find the index first (immutable borrow), then mutate by index.
        let pos = history
            .iter()
            .position(|m| m.role == "user" && m.message_id == Some(event.message_id))
            .or_else(|| history.iter().rposition(|m| m.role == "user"));

        if let Some(pos) = pos {
            debug!(
                "Updating edited message {} in session {}",
                event.message_id, session_id
            );
            history[pos].content = new_content.clone();
            self.conversation_manager
                .save_history(session_id, history)
                .await;
        }

        Ok(())
    }

    /// Process a message deletion event
    ///
    /// Finds the deleted turn by Discord message ID when available; falls back
    /// to removing the most recent user turn for history without IDs.
    /// The assistant reply that immediately follows the deleted turn is also removed.
    pub async fn process_message_deleted(&self, event: &MessageDeletedEvent) -> Result<()> {
        let session_id = &event.metadata.session_id;
        let mut history = self.conversation_manager.get_history(session_id).await;

        if history.is_empty() {
            return Ok(());
        }

        // Prefer exact ID match; fall back to last user message for old history.
        let pos = history
            .iter()
            .position(|m| m.role == "user" && m.message_id == Some(event.message_id))
            .or_else(|| history.iter().rposition(|m| m.role == "user"));

        if let Some(pos) = pos {
            debug!(
                "Removing deleted message {} from session {} history",
                event.message_id, session_id
            );
            history.remove(pos);
            // Also remove the assistant reply that immediately followed it, if any
            if pos < history.len() && history[pos].role == "assistant" {
                history.remove(pos);
            }
            self.conversation_manager
                .save_history(session_id, history)
                .await;
        }

        Ok(())
    }

    /// Process a reaction-add event.
    ///
    /// Supported emoji actions:
    /// - 🔁 / 🔄 — regenerate the last AI response for this user's session
    /// - ❌     — clear the user's conversation history
    pub async fn process_reaction_add(
        &self,
        event: &ReactionAddEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        match event.emoji.name.as_str() {
            "🔁" | "🔄" => self.handle_reaction_regenerate(event, publisher).await,
            "❌" => self.handle_reaction_clear(event, publisher).await,
            other => {
                debug!(
                    "Unhandled reaction '{}' by user {} on message {} in channel {}",
                    other, event.user_id, event.message_id, event.channel_id
                );
                Ok(())
            }
        }
    }

    /// Process a reaction-remove event (no action needed).
    pub async fn process_reaction_remove(
        &self,
        event: &ReactionRemoveEvent,
        _publisher: &MessagePublisher,
    ) -> Result<()> {
        debug!(
            "Reaction '{}' removed by user {} on message {} in channel {}",
            event.emoji.name, event.user_id, event.message_id, event.channel_id
        );
        Ok(())
    }

    /// Regenerate the last AI response for the session that triggered the 🔁 reaction.
    async fn handle_reaction_regenerate(
        &self,
        event: &ReactionAddEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let Some(ref llm_client) = self.llm_client else {
            // Echo mode — nothing to regenerate
            return Ok(());
        };

        let session_id = Self::session_id_for_reaction(event);
        let mut history = self.conversation_manager.get_history(&session_id).await;

        // Need at least one user message to regenerate from
        let Some(last_user_idx) = history.iter().rposition(|m| m.role == "user") else {
            return Ok(());
        };

        // Prior context = everything before the last user turn
        let prior_history = history[..last_user_idx].to_vec();
        let last_user_content = history[last_user_idx].content.clone();

        // Drop the stale assistant reply (if any) so we can append the new one
        history.truncate(last_user_idx + 1);
        self.conversation_manager
            .save_history(&session_id, history)
            .await;

        self.send_typing(event.channel_id, publisher).await?;

        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<String>(64);
        let stream_subject = subjects::agent::message_stream(publisher.prefix());
        // Use a unique sub-session so the streaming bot creates a fresh message
        let regen_session = format!("regen_{}_{}", session_id, event.message_id);

        publisher
            .publish(
                &stream_subject,
                &StreamMessageCommand {
                    channel_id: event.channel_id,
                    content: STREAM_CURSOR.to_string(),
                    is_final: false,
                    session_id: regen_session.clone(),
                    reply_to_message_id: None,
                },
            )
            .await?;

        let llm_client = llm_client.clone();
        let system_prompt = self.system_prompt.clone();

        let llm_handle = tokio::spawn(async move {
            llm_client
                .generate_response_streaming(
                    &system_prompt,
                    &last_user_content,
                    &prior_history,
                    chunk_tx,
                )
                .await
        });

        let mut accumulated = String::new();
        let mut last_publish = Instant::now();

        let stream_deadline = tokio::time::sleep(self.stream_timeout);
        tokio::pin!(stream_deadline);
        let mut timed_out = false;

        loop {
            tokio::select! {
                chunk = chunk_rx.recv() => {
                    match chunk {
                        Some(c) => {
                            accumulated.push_str(&c);
                            if last_publish.elapsed() >= STREAM_PUBLISH_INTERVAL {
                                publisher
                                    .publish(
                                        &stream_subject,
                                        &StreamMessageCommand {
                                            channel_id: event.channel_id,
                                            content: format!("{}{}", accumulated, STREAM_CURSOR),
                                            is_final: false,
                                            session_id: regen_session.clone(),
                                            reply_to_message_id: None,
                                        },
                                    )
                                    .await?;
                                last_publish = Instant::now();
                            }
                        }
                        None => break,
                    }
                }
                _ = &mut stream_deadline => {
                    timed_out = true;
                    llm_handle.abort();
                    break;
                }
            }
        }

        if timed_out {
            warn!(
                "LLM regenerate timed out after {:?} for session {}",
                self.stream_timeout, session_id
            );
            if let Some(ref m) = self.metrics {
                m.inc_llm_errors();
            }
            publisher
                .publish(
                    &stream_subject,
                    &StreamMessageCommand {
                        channel_id: event.channel_id,
                        content: "Sorry, the response timed out. Please try again.".to_string(),
                        is_final: true,
                        session_id: regen_session,
                        reply_to_message_id: None,
                    },
                )
                .await?;
            return Ok(());
        }

        match llm_handle.await? {
            Ok(full_response) => {
                self.conversation_manager
                    .add_message(&session_id, "assistant", &full_response)
                    .await;
                if let Some(ref m) = self.metrics {
                    m.inc_messages_processed();
                }
                publisher
                    .publish(
                        &stream_subject,
                        &StreamMessageCommand {
                            channel_id: event.channel_id,
                            content: full_response,
                            is_final: true,
                            session_id: regen_session,
                            reply_to_message_id: None,
                        },
                    )
                    .await?;
                info!(
                    "Regenerated response for session {} via 🔁 reaction",
                    session_id
                );
            }
            Err(e) => {
                if let Some(ref m) = self.metrics {
                    m.inc_llm_errors();
                }
                warn!("LLM regenerate failed for session {}: {}", session_id, e);
                publisher
                    .publish(
                        &stream_subject,
                        &StreamMessageCommand {
                            channel_id: event.channel_id,
                            content: "Sorry, I couldn't regenerate the response. Please try again."
                                .to_string(),
                            is_final: true,
                            session_id: regen_session,
                            reply_to_message_id: None,
                        },
                    )
                    .await?;
            }
        }

        Ok(())
    }

    /// Clear the user's conversation history when they react with ❌.
    async fn handle_reaction_clear(
        &self,
        event: &ReactionAddEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let session_id = Self::session_id_for_reaction(event);
        self.conversation_manager.clear_session(&session_id).await;

        let cmd = SendMessageCommand {
            channel_id: event.channel_id,
            content: format!("<@{}> Conversation history cleared! 🗑️", event.user_id),
            embeds: vec![],
            reply_to_message_id: None,
            files: vec![],
            components: vec![],
        };
        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;

        info!(
            "Cleared session {} via ❌ reaction from user {}",
            session_id, event.user_id
        );
        Ok(())
    }

    /// Build the session ID that corresponds to a reaction event.
    fn session_id_for_reaction(event: &ReactionAddEvent) -> String {
        let chan_type = if event.guild_id.is_some() {
            ChannelType::GuildText
        } else {
            ChannelType::Dm
        };
        discord_types::session::session_id(
            &chan_type,
            event.channel_id,
            event.guild_id,
            Some(event.user_id),
        )
    }

    /// Process a button / select menu interaction: run LLM on the interaction (or echo in no-LLM mode).
    pub async fn process_component_interaction(
        &self,
        event: &ComponentInteractionEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let session_id = &event.metadata.session_id;
        let channel_id = event.channel_id;

        // Format the interaction as human-readable user input for the LLM
        let user_text = if event.values.is_empty() {
            // Button click
            format!("[Button clicked: {}]", event.custom_id)
        } else {
            // Select menu selection
            format!(
                "[Selected from {}: {}]",
                event.custom_id,
                event.values.join(", ")
            )
        };

        if let Some(ref llm_client) = self.llm_client {
            // Defer — interaction must be acknowledged within 3 seconds
            self.interaction_defer(
                event.interaction_id,
                &event.interaction_token,
                false,
                publisher,
            )
            .await?;

            // Add to conversation history
            self.conversation_manager
                .add_user_message(session_id, &user_text, event.interaction_id)
                .await;

            // Placeholder followup
            let followup_subject = subjects::agent::interaction_followup(publisher.prefix());
            let component_session = format!("component_{}", session_id);
            publisher
                .publish(
                    &followup_subject,
                    &InteractionFollowupCommand {
                        interaction_token: event.interaction_token.clone(),
                        content: Some(STREAM_CURSOR.to_string()),
                        embeds: vec![],
                        ephemeral: false,
                        session_id: Some(component_session.clone()),
                        files: vec![],
                        components: vec![],
                    },
                )
                .await?;

            // Spawn LLM
            let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<String>(64);
            let llm_client = llm_client.clone();
            let history = self.conversation_manager.get_history(session_id).await;
            let system_prompt = self.system_prompt.clone();
            let user_text_owned = user_text.clone();
            let llm_handle = tokio::spawn(async move {
                llm_client
                    .generate_response_streaming(
                        &system_prompt,
                        &user_text_owned,
                        &history,
                        chunk_tx,
                    )
                    .await
            });

            let stream_subject = subjects::agent::message_stream(publisher.prefix());
            let mut accumulated = String::new();
            let mut last_publish = Instant::now();
            let stream_deadline = tokio::time::sleep(self.stream_timeout);
            tokio::pin!(stream_deadline);
            let mut timed_out = false;

            loop {
                tokio::select! {
                    chunk = chunk_rx.recv() => {
                        match chunk {
                            Some(c) => {
                                accumulated.push_str(&c);
                                if last_publish.elapsed() >= STREAM_PUBLISH_INTERVAL {
                                    publisher
                                        .publish(
                                            &stream_subject,
                                            &StreamMessageCommand {
                                                channel_id,
                                                content: format!("{}{}", accumulated, STREAM_CURSOR),
                                                is_final: false,
                                                session_id: component_session.clone(),
                                                reply_to_message_id: None,
                                            },
                                        )
                                        .await?;
                                    last_publish = Instant::now();
                                }
                            }
                            None => break,
                        }
                    }
                    _ = &mut stream_deadline => {
                        timed_out = true;
                        llm_handle.abort();
                        break;
                    }
                }
            }

            if timed_out {
                warn!(
                    "LLM component response timed out for session {}",
                    session_id
                );
                if let Some(ref m) = self.metrics {
                    m.inc_llm_errors();
                }
                publisher
                    .publish(
                        &stream_subject,
                        &StreamMessageCommand {
                            channel_id,
                            content: "Sorry, the response timed out.".to_string(),
                            is_final: true,
                            session_id: component_session,
                            reply_to_message_id: None,
                        },
                    )
                    .await?;
                return Ok(());
            }

            match llm_handle.await? {
                Ok(full_response) => {
                    self.conversation_manager
                        .add_message(session_id, "assistant", &full_response)
                        .await;
                    if let Some(ref m) = self.metrics {
                        m.inc_messages_processed();
                    }
                    publisher
                        .publish(
                            &stream_subject,
                            &StreamMessageCommand {
                                channel_id,
                                content: full_response,
                                is_final: true,
                                session_id: component_session,
                                reply_to_message_id: None,
                            },
                        )
                        .await?;
                }
                Err(e) => {
                    if let Some(ref m) = self.metrics {
                        m.inc_llm_errors();
                    }
                    warn!("LLM component failed for session {}: {}", session_id, e);
                    publisher
                        .publish(
                            &stream_subject,
                            &StreamMessageCommand {
                                channel_id,
                                content: "Sorry, I encountered an error. Please try again."
                                    .to_string(),
                                is_final: true,
                                session_id: component_session,
                                reply_to_message_id: None,
                            },
                        )
                        .await?;
                }
            }
        } else {
            // Echo mode
            self.interaction_respond(
                event.interaction_id,
                &event.interaction_token,
                &format!("Received: {}", user_text),
                true,
                publisher,
            )
            .await?;
        }

        debug!(
            "Processed component interaction '{}' for channel {}",
            event.custom_id, channel_id
        );
        Ok(())
    }

    // ── Helpers ────────────────────────────────────────────────────────────

    /// Build a reply-context prefix so the LLM knows what the user is responding to.
    fn format_reply_context(referenced_content: Option<&str>) -> String {
        match referenced_content {
            Some(c) if !c.is_empty() => format!("[Replying to: \"{}\"]\n\n", c),
            _ => String::new(),
        }
    }

    /// Build an attachment context block to append to the user message so the
    /// LLM is aware of any files the user included.
    fn format_attachments(attachments: &[Attachment]) -> String {
        if attachments.is_empty() {
            return String::new();
        }
        let mut lines = String::from("\n\n[Attachments]");
        for a in attachments {
            let size_kb = a.size / 1024;
            let type_str = a.content_type.as_deref().unwrap_or("unknown type");
            lines.push_str(&format!(
                "\n- {} ({}, {} KB): {}",
                a.filename, type_str, size_kb, a.url
            ));
        }
        lines
    }

    async fn send_typing(&self, channel_id: u64, publisher: &MessagePublisher) -> Result<()> {
        let cmd = TypingCommand { channel_id };
        let subject = subjects::agent::channel_typing(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;
        Ok(())
    }

    async fn interaction_respond(
        &self,
        interaction_id: u64,
        interaction_token: &str,
        content: &str,
        ephemeral: bool,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let cmd = InteractionRespondCommand {
            interaction_id,
            interaction_token: interaction_token.to_string(),
            content: Some(content.to_string()),
            embeds: vec![],
            ephemeral,
            components: vec![],
        };
        let subject = subjects::agent::interaction_respond(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;
        Ok(())
    }

    async fn interaction_defer(
        &self,
        interaction_id: u64,
        interaction_token: &str,
        ephemeral: bool,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let cmd = InteractionDeferCommand {
            interaction_id,
            interaction_token: interaction_token.to_string(),
            ephemeral,
        };
        let subject = subjects::agent::interaction_defer(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;
        Ok(())
    }

    pub async fn process_typing_start(&self, event: &TypingStartEvent) -> Result<()> {
        debug!(
            "User {} typing in channel {}",
            event.user_id, event.channel_id
        );
        Ok(())
    }

    pub async fn process_voice_state_update(&self, event: &VoiceStateUpdateEvent) -> Result<()> {
        debug!(
            "Voice state update for user {} (channel: {:?})",
            event.new_state.user_id, event.new_state.channel_id
        );
        Ok(())
    }

    pub async fn process_guild_create(&self, event: &GuildCreateEvent) -> Result<()> {
        info!(
            "Bot joined guild '{}' (id: {}, {} members)",
            event.guild.name, event.guild.id, event.member_count
        );
        Ok(())
    }

    pub async fn process_guild_update(&self, event: &GuildUpdateEvent) -> Result<()> {
        debug!("Guild '{}' updated (id: {})", event.guild.name, event.guild.id);
        Ok(())
    }

    pub async fn process_guild_delete(&self, event: &GuildDeleteEvent) -> Result<()> {
        info!(
            "Bot left guild {} (unavailable: {})",
            event.guild_id, event.unavailable
        );
        Ok(())
    }

    pub async fn process_channel_create(&self, event: &ChannelCreateEvent) -> Result<()> {
        debug!(
            "Channel created: {:?} (id: {})",
            event.channel.name, event.channel.id
        );
        Ok(())
    }

    pub async fn process_channel_update(&self, event: &ChannelUpdateEvent) -> Result<()> {
        debug!(
            "Channel updated: {:?} (id: {})",
            event.channel.name, event.channel.id
        );
        Ok(())
    }

    pub async fn process_channel_delete(&self, event: &ChannelDeleteEvent) -> Result<()> {
        debug!(
            "Channel {} deleted from guild {}",
            event.channel_id, event.guild_id
        );
        Ok(())
    }

    pub async fn process_role_create(&self, event: &RoleCreateEvent) -> Result<()> {
        debug!(
            "Role '{}' created in guild {}",
            event.role.name, event.guild_id
        );
        Ok(())
    }

    pub async fn process_role_update(&self, event: &RoleUpdateEvent) -> Result<()> {
        debug!(
            "Role '{}' updated in guild {}",
            event.role.name, event.guild_id
        );
        Ok(())
    }

    pub async fn process_role_delete(&self, event: &RoleDeleteEvent) -> Result<()> {
        debug!(
            "Role {} deleted from guild {}",
            event.role_id, event.guild_id
        );
        Ok(())
    }

    pub async fn process_presence_update(&self, event: &PresenceUpdateEvent) -> Result<()> {
        debug!(
            "Presence update: user {} is now '{}' in guild {}",
            event.user_id, event.status, event.guild_id
        );
        Ok(())
    }

    pub async fn process_bot_ready(&self, event: &BotReadyEvent) -> Result<()> {
        info!(
            "Bot ready: {} (id: {}) in {} guilds",
            event.bot_user.username, event.bot_user.id, event.guild_count
        );
        Ok(())
    }

    /// Process a command error event published by the bot.
    pub async fn process_command_error(&self, payload: serde_json::Value) {
        tracing::info!("command error received: {:?}", payload);
    }

    /// Process a guild member update event (nick, role changes, etc.)
    pub async fn process_guild_member_update(&self, event: &GuildMemberUpdateEvent) {
        debug!(
            guild_id = event.guild_id,
            user_id = event.user.id,
            nick = ?event.nick,
            "guild member update"
        );
    }

    /// Respond to autocomplete with a pass-through suggestion.
    /// Autocomplete MUST be responded to within 3 seconds.
    pub async fn process_autocomplete(
        &self,
        event: &AutocompleteEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let choices = if event.current_value.is_empty() {
            vec![]
        } else {
            vec![AutocompleteChoice {
                name: event.current_value.clone(),
                value: event.current_value.clone(),
            }]
        };

        debug!(
            "Autocomplete for '{}' option '{}': responding with {} choices",
            event.command_name,
            event.focused_option,
            choices.len()
        );

        let cmd = AutocompleteRespondCommand {
            interaction_id: event.interaction_id,
            interaction_token: event.interaction_token.clone(),
            choices,
        };
        let subject = subjects::agent::interaction_autocomplete_respond(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;
        Ok(())
    }

    /// Process a modal submission: run LLM on the submitted inputs (or echo in no-LLM mode).
    /// Modals MUST be responded to within 3 seconds.
    pub async fn process_modal_submit(
        &self,
        event: &ModalSubmitEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let session_id = &event.metadata.session_id;
        let channel_id = event.channel_id;

        // Format modal inputs into readable text for the LLM
        let user_text = if event.inputs.is_empty() {
            format!("[Modal submitted: {}]", event.custom_id)
        } else {
            let fields: String = event
                .inputs
                .iter()
                .map(|i| format!("**{}**: {}", i.custom_id, i.value))
                .collect::<Vec<_>>()
                .join("\n");
            format!("[Modal: {}]\n{}", event.custom_id, fields)
        };

        if let Some(ref llm_client) = self.llm_client {
            // Defer first — modal must be acknowledged within 3 seconds
            self.interaction_defer(
                event.interaction_id,
                &event.interaction_token,
                false,
                publisher,
            )
            .await?;

            // Add to conversation history
            self.conversation_manager
                .add_user_message(session_id, &user_text, event.interaction_id)
                .await;

            // Send placeholder followup
            let followup_subject = subjects::agent::interaction_followup(publisher.prefix());
            let modal_session = format!("modal_{}", session_id);
            publisher
                .publish(
                    &followup_subject,
                    &InteractionFollowupCommand {
                        interaction_token: event.interaction_token.clone(),
                        content: Some(STREAM_CURSOR.to_string()),
                        embeds: vec![],
                        ephemeral: false,
                        session_id: Some(modal_session.clone()),
                        files: vec![],
                        components: vec![],
                    },
                )
                .await?;

            // Spawn LLM task
            let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<String>(64);
            let llm_client = llm_client.clone();
            let history = self.conversation_manager.get_history(session_id).await;
            let system_prompt = self.system_prompt.clone();
            let user_text_owned = user_text.clone();
            let llm_handle = tokio::spawn(async move {
                llm_client
                    .generate_response_streaming(
                        &system_prompt,
                        &user_text_owned,
                        &history,
                        chunk_tx,
                    )
                    .await
            });

            let stream_subject = subjects::agent::message_stream(publisher.prefix());
            let mut accumulated = String::new();
            let mut last_publish = Instant::now();
            let stream_deadline = tokio::time::sleep(self.stream_timeout);
            tokio::pin!(stream_deadline);
            let mut timed_out = false;

            loop {
                tokio::select! {
                    chunk = chunk_rx.recv() => {
                        match chunk {
                            Some(c) => {
                                accumulated.push_str(&c);
                                if last_publish.elapsed() >= STREAM_PUBLISH_INTERVAL {
                                    publisher
                                        .publish(
                                            &stream_subject,
                                            &StreamMessageCommand {
                                                channel_id,
                                                content: format!("{}{}", accumulated, STREAM_CURSOR),
                                                is_final: false,
                                                session_id: modal_session.clone(),
                                                reply_to_message_id: None,
                                            },
                                        )
                                        .await?;
                                    last_publish = Instant::now();
                                }
                            }
                            None => break,
                        }
                    }
                    _ = &mut stream_deadline => {
                        timed_out = true;
                        llm_handle.abort();
                        break;
                    }
                }
            }

            if timed_out {
                warn!("LLM modal response timed out for session {}", session_id);
                if let Some(ref m) = self.metrics {
                    m.inc_llm_errors();
                }
                publisher
                    .publish(
                        &stream_subject,
                        &StreamMessageCommand {
                            channel_id,
                            content: "Sorry, the response timed out.".to_string(),
                            is_final: true,
                            session_id: modal_session,
                            reply_to_message_id: None,
                        },
                    )
                    .await?;
                return Ok(());
            }

            match llm_handle.await? {
                Ok(full_response) => {
                    self.conversation_manager
                        .add_message(session_id, "assistant", &full_response)
                        .await;
                    if let Some(ref m) = self.metrics {
                        m.inc_messages_processed();
                    }
                    publisher
                        .publish(
                            &stream_subject,
                            &StreamMessageCommand {
                                channel_id,
                                content: full_response,
                                is_final: true,
                                session_id: modal_session,
                                reply_to_message_id: None,
                            },
                        )
                        .await?;
                }
                Err(e) => {
                    if let Some(ref m) = self.metrics {
                        m.inc_llm_errors();
                    }
                    warn!("LLM modal failed for session {}: {}", session_id, e);
                    publisher
                        .publish(
                            &stream_subject,
                            &StreamMessageCommand {
                                channel_id,
                                content: "Sorry, I encountered an error. Please try again."
                                    .to_string(),
                                is_final: true,
                                session_id: modal_session,
                                reply_to_message_id: None,
                            },
                        )
                        .await?;
                }
            }
        } else {
            // Echo mode: acknowledge with formatted inputs
            self.interaction_respond(
                event.interaction_id,
                &event.interaction_token,
                &format!("Received modal: {}", user_text),
                true,
                publisher,
            )
            .await?;
        }

        debug!(
            "Processed modal submit '{}' for user {}",
            event.custom_id, event.user.id
        );
        Ok(())
    }
}

impl Default for MessageProcessor {
    fn default() -> Self {
        Self::new(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            20,
            Duration::from_secs(120),
        )
    }
}
