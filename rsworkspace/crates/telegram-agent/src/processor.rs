//! Message processor - handles business logic for different message types

use telegram_types::events::{MessageTextEvent, MessagePhotoEvent, CommandEvent, CallbackQueryEvent};
use telegram_types::commands::{SendMessageCommand, AnswerCallbackCommand, SendChatActionCommand, ChatAction};
use telegram_nats::{MessagePublisher, subjects};
use tracing::{debug, info};
use anyhow::Result;

/// Message processor
pub struct MessageProcessor {
    // In the future, this will hold:
    // - LLM client
    // - Conversation state
    // - Memory/context
}

impl MessageProcessor {
    /// Create a new message processor
    pub fn new() -> Self {
        Self {}
    }

    /// Process a text message
    pub async fn process_text_message(
        &self,
        event: &MessageTextEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let chat_id = event.message.chat.id;

        // Send typing indicator
        self.send_typing_indicator(chat_id, publisher).await?;

        // For now, echo the message back
        // TODO: Replace with LLM integration
        let response_text = format!("You said: {}", event.text);

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

        let response_text = match event.command.as_str() {
            "start" => {
                "👋 Welcome to the Telegram AI Agent!\n\n\
                I'm powered by NATS and can help you with various tasks.\n\n\
                Send me a message to get started!"
            }
            "help" => {
                "Available commands:\n\n\
                /start - Start the bot\n\
                /help - Show this help message\n\
                /status - Show bot status\n\n\
                Just send me any text and I'll respond!"
            }
            "status" => {
                "✅ Bot is running and connected to NATS.\n\
                Ready to process messages!"
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
        Self::new()
    }
}
