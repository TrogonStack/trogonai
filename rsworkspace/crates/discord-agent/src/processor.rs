//! Message processor - handles business logic for different Discord event types

use anyhow::Result;
use discord_nats::{subjects, MessagePublisher};
use discord_types::{
    events::{ComponentInteractionEvent, MessageCreatedEvent, SlashCommandEvent},
    InteractionDeferCommand, InteractionFollowupCommand, InteractionRespondCommand,
    SendMessageCommand, TypingCommand,
};
use tracing::{debug, info, warn};

use crate::conversation::ConversationManager;
use crate::llm::{ClaudeClient, ClaudeConfig};

/// Message processor
pub struct MessageProcessor {
    llm_client: Option<ClaudeClient>,
    conversation_manager: ConversationManager,
}

impl MessageProcessor {
    /// Create a new message processor
    pub fn new(
        llm_config: Option<ClaudeConfig>,
        conversation_kv: Option<async_nats::jetstream::kv::Store>,
    ) -> Self {
        let conversation_manager = match conversation_kv {
            Some(kv) => ConversationManager::with_kv(kv),
            None => ConversationManager::new(),
        };
        Self {
            llm_client: llm_config.map(ClaudeClient::new),
            conversation_manager,
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

            let system_prompt = "You are a helpful AI assistant in a Discord server. \
                Be concise, friendly, and helpful. Keep responses under 500 words unless \
                the user specifically asks for more detail.";

            self.conversation_manager
                .add_message(session_id, "user", content)
                .await;

            match llm_client
                .generate_response(system_prompt, content, &history)
                .await
            {
                Ok(response) => {
                    self.conversation_manager
                        .add_message(session_id, "assistant", &response)
                        .await;

                    let cmd = SendMessageCommand {
                        channel_id,
                        content: response,
                        embeds: vec![],
                        reply_to_message_id: Some(message_id),
                    };
                    let subject = subjects::agent::message_send(publisher.prefix());
                    publisher.publish(&subject, &cmd).await?;
                    debug!("Sent LLM response to channel {}", channel_id);
                }
                Err(e) => {
                    warn!("LLM generation failed for session {}: {}", session_id, e);
                    self.send_error_reply(channel_id, Some(message_id), publisher)
                        .await?;
                }
            }
        } else {
            // Echo mode
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
                    // Defer first — LLM calls take time
                    self.interaction_defer(
                        event.interaction_id,
                        &event.interaction_token,
                        false,
                        publisher,
                    )
                    .await?;

                    let history = self.conversation_manager.get_history(session_id).await;
                    let system_prompt = "You are a helpful AI assistant in a Discord server. \
                        Be concise, friendly, and helpful.";

                    self.conversation_manager
                        .add_message(session_id, "user", &question)
                        .await;

                    match llm_client
                        .generate_response(system_prompt, &question, &history)
                        .await
                    {
                        Ok(response) => {
                            self.conversation_manager
                                .add_message(session_id, "assistant", &response)
                                .await;

                            let cmd = InteractionFollowupCommand {
                                interaction_token: event.interaction_token.clone(),
                                content: Some(response),
                                embeds: vec![],
                                ephemeral: false,
                            };
                            let subject = subjects::agent::interaction_followup(publisher.prefix());
                            publisher.publish(&subject, &cmd).await?;
                        }
                        Err(e) => {
                            warn!("LLM failed for /ask in session {}: {}", session_id, e);
                            let cmd = InteractionFollowupCommand {
                                interaction_token: event.interaction_token.clone(),
                                content: Some(
                                    "Sorry, I encountered an error. Please try again.".to_string(),
                                ),
                                embeds: vec![],
                                ephemeral: true,
                            };
                            let subject = subjects::agent::interaction_followup(publisher.prefix());
                            publisher.publish(&subject, &cmd).await?;
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

    async fn send_error_reply(
        &self,
        channel_id: u64,
        reply_to_message_id: Option<u64>,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let cmd = SendMessageCommand {
            channel_id,
            content: "Sorry, I encountered an error generating a response. Please try again."
                .to_string(),
            embeds: vec![],
            reply_to_message_id,
        };
        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;
        Ok(())
    }
}

impl Default for MessageProcessor {
    fn default() -> Self {
        Self::new(None, None)
    }
}
