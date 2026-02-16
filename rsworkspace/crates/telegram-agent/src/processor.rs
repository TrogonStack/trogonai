//! Message processor - handles business logic for different message types

use telegram_types::events::{MessageTextEvent, MessagePhotoEvent, CommandEvent, CallbackQueryEvent};
use telegram_types::commands::{SendMessageCommand, AnswerCallbackCommand, SendChatActionCommand, ChatAction};
use telegram_nats::{MessagePublisher, subjects};
use tracing::{debug, info, warn};
use anyhow::Result;

use crate::llm::{ClaudeClient, ClaudeConfig};
use crate::conversation::ConversationManager;

/// Message processor
pub struct MessageProcessor {
    llm_client: Option<ClaudeClient>,
    conversation_manager: ConversationManager,
}

impl MessageProcessor {
    /// Create a new message processor
    pub fn new(llm_config: Option<ClaudeConfig>) -> Self {
        Self {
            llm_client: llm_config.map(ClaudeClient::new),
            conversation_manager: ConversationManager::new(),
        }
    }

    /// Process a text message
    pub async fn process_text_message(
        &self,
        event: &MessageTextEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let chat_id = event.message.chat.id;
        let session_id = &event.metadata.session_id;

        // Send typing indicator
        self.send_typing_indicator(chat_id, publisher).await?;

        // Generate response
        let response_text = if let Some(ref llm_client) = self.llm_client {
            // LLM mode: Generate intelligent response
            debug!("Generating LLM response for session {}", session_id);

            // Get conversation history
            let history = self.conversation_manager.get_history(session_id).await;

            // System prompt
            let system_prompt = "You are a helpful AI assistant in a Telegram chat. \
                Be concise, friendly, and helpful. Keep responses under 500 words unless \
                the user specifically asks for more detail.";

            // Generate response
            match llm_client.generate_response(system_prompt, &event.text, &history).await {
                Ok(response) => {
                    // Add to conversation history
                    self.conversation_manager.add_message(session_id, "user", &event.text).await;
                    self.conversation_manager.add_message(session_id, "assistant", &response).await;
                    response
                }
                Err(e) => {
                    warn!("LLM generation failed: {}", e);
                    "Sorry, I encountered an error generating a response. Please try again.".to_string()
                }
            }
        } else {
            // Echo mode: Simple echo response
            format!("You said: {}", event.text)
        };

        // Send response
        let command = SendMessageCommand {
            chat_id,
            text: response_text,
            parse_mode: None,
            reply_to_message_id: Some(event.message.message_id),
            reply_markup: None,
        };

        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &command).await?;

        debug!("Sent response to chat {}", chat_id);
        Ok(())
    }

    /// Process a photo message
    pub async fn process_photo_message(
        &self,
        event: &MessagePhotoEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let chat_id = event.message.chat.id;

        // Send typing indicator
        self.send_typing_indicator(chat_id, publisher).await?;

        // Respond to photo
        let response_text = if let Some(ref caption) = event.caption {
            format!("I received your photo with caption: {}", caption)
        } else {
            "I received your photo!".to_string()
        };

        let command = SendMessageCommand {
            chat_id,
            text: response_text,
            parse_mode: None,
            reply_to_message_id: Some(event.message.message_id),
            reply_markup: None,
        };

        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &command).await?;

        debug!("Sent response to photo in chat {}", chat_id);
        Ok(())
    }

    /// Process a command
    pub async fn process_command(
        &self,
        event: &CommandEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let chat_id = event.message.chat.id;
        let session_id = &event.metadata.session_id;

        let response_text = match event.command.as_str() {
            "start" => {
                if self.llm_client.is_some() {
                    "👋 Welcome to the Telegram AI Agent!\n\n\
                    I'm powered by Claude AI and can help you with:\n\
                    • Answering questions\n\
                    • Writing and editing text\n\
                    • Analyzing images\n\
                    • General assistance\n\n\
                    Just send me a message to get started!"
                } else {
                    "👋 Welcome to the Telegram AI Agent!\n\n\
                    I'm running in echo mode (LLM disabled).\n\n\
                    Send me a message and I'll echo it back!"
                }
            }
            "help" => {
                "Available commands:\n\n\
                /start - Start the bot\n\
                /help - Show this help message\n\
                /status - Show bot status\n\
                /clear - Clear conversation history\n\n\
                Just send me any text and I'll respond!"
            }
            "status" => {
                let active_sessions = self.conversation_manager.active_sessions().await;
                let mode = if self.llm_client.is_some() {
                    "LLM (Claude AI)"
                } else {
                    "Echo mode"
                };
                &format!(
                    "✅ Bot Status\n\n\
                    Mode: {}\n\
                    Active sessions: {}\n\
                    Ready to process messages!",
                    mode, active_sessions
                )
            }
            "clear" => {
                self.conversation_manager.clear_session(session_id).await;
                "✅ Conversation history cleared!"
            }
            _ => {
                "Unknown command. Use /help to see available commands."
            }
        };

        let command = SendMessageCommand {
            chat_id,
            text: response_text.to_string(),
            parse_mode: None,
            reply_to_message_id: Some(event.message.message_id),
            reply_markup: None,
        };

        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &command).await?;

        info!("Processed command /{} for chat {}", event.command, chat_id);
        Ok(())
    }

    /// Process a callback query (button click)
    pub async fn process_callback(
        &self,
        event: &CallbackQueryEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        // Answer the callback query first
        let answer_command = AnswerCallbackCommand {
            callback_query_id: event.callback_query_id.clone(),
            text: Some("Button clicked!".to_string()),
            show_alert: Some(false),
        };

        let subject = subjects::agent::callback_answer(publisher.prefix());
        publisher.publish(&subject, &answer_command).await?;

        // Send a message with the callback data
        let response_text = format!("You clicked: {}", event.data);

        let message_command = SendMessageCommand {
            chat_id: event.chat.id,
            text: response_text,
            parse_mode: None,
            reply_to_message_id: event.message_id,
            reply_markup: None,
        };

        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &message_command).await?;

        debug!("Processed callback for chat {}", event.chat.id);
        Ok(())
    }

    /// Send typing indicator
    async fn send_typing_indicator(
        &self,
        chat_id: i64,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let command = SendChatActionCommand {
            chat_id,
            action: ChatAction::Typing,
        };

        let subject = subjects::agent::chat_action(publisher.prefix());
        publisher.publish(&subject, &command).await?;

        Ok(())
    }
}

impl Default for MessageProcessor {
    fn default() -> Self {
        Self::new(None)
    }
}
