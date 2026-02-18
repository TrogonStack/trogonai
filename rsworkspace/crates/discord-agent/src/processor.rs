//! Message processor - handles business logic for different Discord event types

use anyhow::Result;
use discord_nats::{subjects, MessagePublisher};
use discord_types::{
    events::{
        ComponentInteractionEvent, GuildMemberAddEvent, GuildMemberRemoveEvent,
        MessageCreatedEvent, MessageDeletedEvent, MessageUpdatedEvent, ReactionAddEvent,
        ReactionRemoveEvent, SlashCommandEvent,
    },
    types::Attachment,
    InteractionDeferCommand, InteractionFollowupCommand, InteractionRespondCommand,
    SendMessageCommand, StreamMessageCommand, TypingCommand,
};
use tokio::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::conversation::ConversationManager;
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
}

impl MessageProcessor {
    /// Create a new message processor
    pub fn new(
        llm_config: Option<ClaudeConfig>,
        conversation_kv: Option<async_nats::jetstream::kv::Store>,
        system_prompt: Option<String>,
        welcome: Option<WelcomeConfig>,
        farewell: Option<WelcomeConfig>,
        conversation_ttl: Option<Duration>,
    ) -> Self {
        let conversation_manager = match conversation_kv {
            Some(kv) => ConversationManager::with_kv(kv),
            None => ConversationManager::new(),
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

            self.conversation_manager
                .add_message(session_id, "user", content)
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
            // Append attachment metadata so the LLM is aware of any uploaded files
            let attachment_ctx = Self::format_attachments(&event.message.attachments);
            let content_owned = format!("{}{}", content, attachment_ctx);

            let llm_handle = tokio::spawn(async move {
                llm_client
                    .generate_response_streaming(&system_prompt, &content_owned, &history, chunk_tx)
                    .await
            });

            let mut accumulated = String::new();
            let mut last_publish = Instant::now();

            // Consume chunks and publish throttled updates

            while let Some(chunk) = chunk_rx.recv().await {
                accumulated.push_str(&chunk);
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

                    debug!("Streamed LLM response to channel {}", channel_id);
                }
                Err(e) => {
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
                    `/ask <question>` — Ask the AI a question\n\n\
                    You can also just send a message in any allowed channel!"
                } else {
                    "**Discord AI Agent** *(echo mode)*\n\n\
                    Available commands:\n\
                    `/ping` — Check if the bot is alive\n\
                    `/help` — Show this message\n\
                    `/status` — Show agent status\n\
                    `/clear` — Clear your conversation history\n\n\
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
                let text = format!(
                    "**Agent Status**\nMode: {}\nActive sessions: {}\nReady ✅",
                    mode, active
                );
                self.interaction_respond(
                    event.interaction_id,
                    &event.interaction_token,
                    &text,
                    false,
                    publisher,
                )
                .await?;
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

                    while let Some(chunk) = chunk_rx.recv().await {
                        accumulated.push_str(&chunk);
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

                    match llm_handle.await? {
                        Ok(full_response) => {
                            self.conversation_manager
                                .add_message(session_id, "assistant", &full_response)
                                .await;

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
    /// Updates the most recent matching user message in conversation history so the
    /// LLM sees the corrected content in future turns.
    pub async fn process_message_updated(&self, event: &MessageUpdatedEvent) -> Result<()> {
        let Some(ref new_content) = event.new_content else {
            return Ok(());
        };

        let session_id = &event.metadata.session_id;
        let mut history = self.conversation_manager.get_history(session_id).await;

        // Walk backwards to find the most recent user message and update it
        if let Some(entry) = history.iter_mut().rev().find(|m| m.role == "user") {
            debug!(
                "Updating edited message in session {} (msg {})",
                session_id, event.message_id
            );
            entry.content = new_content.clone();
            // Persist the updated history
            self.conversation_manager
                .save_history(session_id, history)
                .await;
        }

        Ok(())
    }

    /// Process a message deletion event
    ///
    /// Removes the most recent user message from conversation history when the
    /// original message is deleted, keeping the context clean.
    pub async fn process_message_deleted(&self, event: &MessageDeletedEvent) -> Result<()> {
        let session_id = &event.metadata.session_id;
        let mut history = self.conversation_manager.get_history(session_id).await;

        if history.is_empty() {
            return Ok(());
        }

        // Remove the last user message (we can't match by message ID since history
        // doesn't store Discord message IDs, so we remove the most recent user turn)
        if let Some(pos) = history.iter().rposition(|m| m.role == "user") {
            debug!(
                "Removing deleted message from session {} history (msg {})",
                session_id, event.message_id
            );
            history.remove(pos);
            // Also remove the assistant reply that followed it, if any
            if pos < history.len() && history[pos].role == "assistant" {
                history.remove(pos);
            }
            self.conversation_manager
                .save_history(session_id, history)
                .await;
        }

        Ok(())
    }

    /// Process a reaction-add event (no-op unless overridden by a subclass)
    pub async fn process_reaction_add(&self, event: &ReactionAddEvent) -> Result<()> {
        debug!(
            "Reaction {} added by user {} on message {} in channel {}",
            event.emoji.name, event.user_id, event.message_id, event.channel_id
        );
        Ok(())
    }

    /// Process a reaction-remove event
    pub async fn process_reaction_remove(&self, event: &ReactionRemoveEvent) -> Result<()> {
        debug!(
            "Reaction {} removed by user {} on message {} in channel {}",
            event.emoji.name, event.user_id, event.message_id, event.channel_id
        );
        Ok(())
    }

    /// Process a button / select menu interaction
    pub async fn process_component_interaction(
        &self,
        event: &ComponentInteractionEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        // Acknowledge the interaction and echo the custom_id
        let text = format!("You interacted with: `{}`", event.custom_id);
        self.interaction_respond(
            event.interaction_id,
            &event.interaction_token,
            &text,
            true,
            publisher,
        )
        .await?;

        debug!(
            "Processed component interaction '{}' for channel {}",
            event.custom_id, event.channel_id
        );
        Ok(())
    }

    // ── Helpers ────────────────────────────────────────────────────────────

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
}

impl Default for MessageProcessor {
    fn default() -> Self {
        Self::new(None, None, None, None, None, None)
    }
}
