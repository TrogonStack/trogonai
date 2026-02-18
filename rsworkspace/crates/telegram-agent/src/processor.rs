//! Message processor - handles business logic for different message types

use anyhow::Result;
use telegram_nats::{subjects, MessagePublisher};
use telegram_types::commands::{
    AnswerCallbackCommand, ChatAction, CommandMetadata, SendChatActionCommand, SendMessageCommand,
    StreamMessageCommand,
};
use telegram_types::events::{
    CallbackQueryEvent, CommandEvent, MessagePhotoEvent, MessageTextEvent,
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

    /// Process a text message
    pub async fn process_text_message(
        &self,
        event: &MessageTextEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let chat_id = event.message.chat.id;
        let session_id = &event.metadata.session_id;
        let message_thread_id = event.message.message_thread_id;

        // Metadata factory: captures session_id and causation (triggering event_id)
        let causation_id = event.metadata.event_id.to_string();
        let meta =
            || CommandMetadata::new().with_causation(session_id.as_str(), causation_id.as_str());

        // Send typing indicator
        self.send_typing_indicator(chat_id, publisher).await?;

        if let Some(ref llm_client) = self.llm_client {
            let history = self.conversation_manager.get_history(session_id).await;

            let system_prompt = "You are a helpful AI assistant in a Telegram chat. \
                Be concise, friendly, and helpful. Keep responses under 500 words unless \
                the user specifically asks for more detail.";

            // Record user message before streaming starts
            self.conversation_manager
                .add_message(session_id, "user", &event.text)
                .await;

            match llm_client
                .generate_response_streaming(system_prompt, &event.text, &history)
                .await
            {
                Ok(mut rx) => {
                    let stream_subject = subjects::agent::message_stream(publisher.prefix());
                    let mut accumulated = String::new();

                    while let Some(chunk_result) = rx.recv().await {
                        match chunk_result {
                            Ok(chunk) => {
                                accumulated.push_str(&chunk);

                                let cmd = StreamMessageCommand {
                                    chat_id,
                                    message_id: None,
                                    text: accumulated.clone(),
                                    parse_mode: None,
                                    is_final: false,
                                    session_id: Some(session_id.clone()),
                                    message_thread_id,
                                };
                                publisher
                                    .publish_command(&stream_subject, &cmd, meta())
                                    .await?;
                            }
                            Err(e) => {
                                warn!("Streaming chunk error for session {}: {}", session_id, e);
                            }
                        }
                    }

                    if !accumulated.is_empty() {
                        // Send final chunk to signal completion
                        let cmd = StreamMessageCommand {
                            chat_id,
                            message_id: None,
                            text: accumulated.clone(),
                            parse_mode: None,
                            is_final: true,
                            session_id: Some(session_id.clone()),
                            message_thread_id,
                        };
                        publisher
                            .publish_command(&stream_subject, &cmd, meta())
                            .await?;

                        self.conversation_manager
                            .add_message(session_id, "assistant", &accumulated)
                            .await;
                        debug!("Streamed response to chat {}", chat_id);
                    } else {
                        warn!(
                            "LLM returned empty streaming response for session {}",
                            session_id
                        );
                        self.send_error_reply(
                            chat_id,
                            event.message.message_id,
                            message_thread_id,
                            publisher,
                        )
                        .await?;
                    }
                }
                Err(e) => {
                    warn!("LLM streaming failed for session {}: {}", session_id, e);
                    self.send_error_reply(
                        chat_id,
                        event.message.message_id,
                        message_thread_id,
                        publisher,
                    )
                    .await?;
                }
            }
        } else {
            // Echo mode
            let cmd = SendMessageCommand {
                chat_id,
                text: format!("You said: {}", event.text),
                parse_mode: None,
                reply_to_message_id: Some(event.message.message_id),
                reply_markup: None,
                message_thread_id,
            };
            let subject = subjects::agent::message_send(publisher.prefix());
            publisher.publish_command(&subject, &cmd, meta()).await?;
        }

        Ok(())
    }

    /// Process a photo message
    pub async fn process_photo_message(
        &self,
        event: &MessagePhotoEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let chat_id = event.message.chat.id;
        let session_id = &event.metadata.session_id;
        let message_thread_id = event.message.message_thread_id;

        // Send typing indicator
        self.send_typing_indicator(chat_id, publisher).await?;

        let response_text = if let Some(ref llm_client) = self.llm_client {
            // Build a description of the photo for the conversation context
            let photo_description = match &event.caption {
                Some(caption) => format!("[User sent a photo with caption: \"{}\"]", caption),
                None => "[User sent a photo without caption]".to_string(),
            };

            let history = self.conversation_manager.get_history(session_id).await;
            self.conversation_manager
                .add_message(session_id, "user", &photo_description)
                .await;

            let system_prompt = "You are a helpful AI assistant in a Telegram chat. \
                Be concise, friendly, and helpful.";

            match llm_client
                .generate_response(system_prompt, &photo_description, &history)
                .await
            {
                Ok(response) => {
                    self.conversation_manager
                        .add_message(session_id, "assistant", &response)
                        .await;
                    response
                }
                Err(e) => {
                    warn!(
                        "LLM generation failed for photo in session {}: {}",
                        session_id, e
                    );
                    "Sorry, I encountered an error processing your photo. Please try again."
                        .to_string()
                }
            }
        } else {
            match &event.caption {
                Some(caption) => format!("I received your photo with caption: {}", caption),
                None => "I received your photo!".to_string(),
            }
        };

        let meta = CommandMetadata::new().with_causation(
            session_id.as_str(),
            event.metadata.event_id.to_string().as_str(),
        );

        let cmd = SendMessageCommand {
            chat_id,
            text: response_text,
            parse_mode: None,
            reply_to_message_id: Some(event.message.message_id),
            reply_markup: None,
            message_thread_id,
        };

        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish_command(&subject, &cmd, meta).await?;

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
        let message_thread_id = event.message.message_thread_id;

        let response_text = match event.command.as_str() {
            "start" => {
                if self.llm_client.is_some() {
                    "Welcome to the Telegram AI Agent!\n\n\
                    I'm powered by Claude AI and can help you with:\n\
                    - Answering questions\n\
                    - Writing and editing text\n\
                    - General assistance\n\n\
                    Just send me a message to get started!"
                } else {
                    "Welcome to the Telegram AI Agent!\n\n\
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
                    "Bot Status\n\nMode: {}\nActive sessions: {}\nReady to process messages!",
                    mode, active_sessions
                )
            }
            "clear" => {
                self.conversation_manager.clear_session(session_id).await;
                "Conversation history cleared!"
            }
            _ => "Unknown command. Use /help to see available commands.",
        };

        let meta = CommandMetadata::new().with_causation(
            session_id.as_str(),
            event.metadata.event_id.to_string().as_str(),
        );

        let cmd = SendMessageCommand {
            chat_id,
            text: response_text.to_string(),
            parse_mode: None,
            reply_to_message_id: Some(event.message.message_id),
            reply_markup: None,
            message_thread_id,
        };

        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish_command(&subject, &cmd, meta).await?;

        info!("Processed command /{} for chat {}", event.command, chat_id);
        Ok(())
    }

    /// Process a callback query (button click)
    pub async fn process_callback(
        &self,
        event: &CallbackQueryEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let meta = || {
            CommandMetadata::new().with_causation(
                event.metadata.session_id.as_str(),
                event.metadata.event_id.to_string().as_str(),
            )
        };

        // Answer the callback query first
        let answer_command = AnswerCallbackCommand {
            callback_query_id: event.callback_query_id.clone(),
            text: Some("Button clicked!".to_string()),
            show_alert: Some(false),
        };

        let subject = subjects::agent::callback_answer(publisher.prefix());
        publisher
            .publish_command(&subject, &answer_command, meta())
            .await?;

        // Send a message with the callback data
        let message_command = SendMessageCommand {
            chat_id: event.chat.id,
            text: format!("You clicked: {}", event.data),
            parse_mode: None,
            reply_to_message_id: event.message_id,
            reply_markup: None,
            message_thread_id: None,
        };

        let subject = subjects::agent::message_send(publisher.prefix());
        publisher
            .publish_command(&subject, &message_command, meta())
            .await?;

        debug!("Processed callback for chat {}", event.chat.id);
        Ok(())
    }

    /// Process an inline query
    pub async fn process_inline_query(
        &self,
        event: &telegram_types::events::InlineQueryEvent,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        use telegram_types::commands::{
            AnswerInlineQueryCommand, InlineQueryResult, InlineQueryResultArticle,
            InputMessageContent,
        };

        info!("Processing inline query: {}", event.query);

        let results = if let Some(ref llm_client) = self.llm_client {
            let system_prompt = "You are a helpful AI assistant. Provide a concise, helpful response to the user's query.";

            let response = match llm_client
                .generate_response(system_prompt, &event.query, &[])
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    warn!("LLM generation failed for inline query: {}", e);
                    format!("I couldn't process your query: {}", event.query)
                }
            };

            vec![InlineQueryResult::Article(InlineQueryResultArticle {
                id: "1".to_string(),
                title: format!("AI Response: {}", truncate(&event.query, 50)),
                description: Some(truncate(&response, 100)),
                input_message_content: InputMessageContent {
                    message_text: response,
                    parse_mode: None,
                },
                thumb_url: None,
            })]
        } else {
            vec![
                InlineQueryResult::Article(InlineQueryResultArticle {
                    id: "1".to_string(),
                    title: format!("Echo: {}", event.query),
                    description: Some("Tap to send this query as a message".to_string()),
                    input_message_content: InputMessageContent {
                        message_text: format!("You searched for: {}", event.query),
                        parse_mode: None,
                    },
                    thumb_url: None,
                }),
                InlineQueryResult::Article(InlineQueryResultArticle {
                    id: "2".to_string(),
                    title: "Uppercase".to_string(),
                    description: Some(event.query.to_uppercase()),
                    input_message_content: InputMessageContent {
                        message_text: event.query.to_uppercase(),
                        parse_mode: None,
                    },
                    thumb_url: None,
                }),
                InlineQueryResult::Article(InlineQueryResultArticle {
                    id: "3".to_string(),
                    title: "Lowercase".to_string(),
                    description: Some(event.query.to_lowercase()),
                    input_message_content: InputMessageContent {
                        message_text: event.query.to_lowercase(),
                        parse_mode: None,
                    },
                    thumb_url: None,
                }),
            ]
        };

        let answer_command = AnswerInlineQueryCommand {
            inline_query_id: event.inline_query_id.clone(),
            results,
            cache_time: Some(300),
            is_personal: Some(true),
            next_offset: None,
        };

        let meta = CommandMetadata::new().with_causation(
            event.metadata.session_id.as_str(),
            event.metadata.event_id.to_string().as_str(),
        );
        let subject = subjects::agent::inline_answer(publisher.prefix());
        publisher
            .publish_command(&subject, &answer_command, meta)
            .await?;

        debug!("Answered inline query {}", event.inline_query_id);
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
            message_thread_id: None,
        };

        let subject = subjects::agent::chat_action(publisher.prefix());
        publisher.publish(&subject, &command).await?;

        Ok(())
    }

    /// Send a generic error reply message
    async fn send_error_reply(
        &self,
        chat_id: i64,
        reply_to_message_id: i32,
        message_thread_id: Option<i32>,
        publisher: &MessagePublisher,
    ) -> Result<()> {
        let cmd = SendMessageCommand {
            chat_id,
            text: "Sorry, I encountered an error generating a response. Please try again."
                .to_string(),
            parse_mode: None,
            reply_to_message_id: Some(reply_to_message_id),
            reply_markup: None,
            message_thread_id,
        };
        let subject = subjects::agent::message_send(publisher.prefix());
        publisher.publish(&subject, &cmd).await?;
        Ok(())
    }
}

/// Truncate a string to a maximum length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

impl Default for MessageProcessor {
    fn default() -> Self {
        Self::new(None, None)
    }
}
