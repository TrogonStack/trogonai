//! Bridge between Telegram and NATS
//!
//! Converts Telegram updates into NATS events

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod bridge_tests;

use async_nats::{Client, jetstream::kv::Store};
use teloxide::types::{Message, CallbackQuery};
use telegram_nats::{MessagePublisher, subjects};
use telegram_types::{
    AccessConfig, SessionId,
    events::*,
    chat::{Chat, ChatType, User, Message as TgMessage, PhotoSize, FileInfo},
};
use tracing::{debug, info, warn};
use anyhow::Result;

use crate::session::SessionManager;

/// Telegram to NATS bridge
#[derive(Clone)]
pub struct TelegramBridge {
    publisher: MessagePublisher,
    pub access_config: AccessConfig,
    session_manager: SessionManager,
}

impl TelegramBridge {
    /// Create a new bridge
    pub fn new(client: Client, prefix: String, access_config: AccessConfig, kv: Store) -> Self {
        Self {
            publisher: MessagePublisher::new(client, prefix),
            access_config,
            session_manager: SessionManager::new(kv),
        }
    }

    /// Check if a user has access for DMs
    pub fn check_dm_access(&self, user_id: i64) -> bool {
        self.access_config.can_access_dm(user_id)
    }

    /// Check if a group has access
    pub fn check_group_access(&self, group_id: i64) -> bool {
        self.access_config.can_access_group(group_id)
    }

    /// Convert teloxide Message to our Message type
    fn convert_message(&self, msg: &Message) -> TgMessage {
        TgMessage {
            message_id: msg.id.0,
            date: msg.date.timestamp(),
            chat: self.convert_chat(&msg.chat),
            from: msg.from.as_ref().map(|u| self.convert_user(u)),
        }
    }

    /// Convert teloxide Chat to our Chat type
    fn convert_chat(&self, chat: &teloxide::types::Chat) -> Chat {
        let chat_type = match chat.kind {
            teloxide::types::ChatKind::Public(ref p) => match p.kind {
                teloxide::types::PublicChatKind::Channel(_) => ChatType::Channel,
                teloxide::types::PublicChatKind::Group => ChatType::Group,
                teloxide::types::PublicChatKind::Supergroup(_) => ChatType::Supergroup,
            },
            teloxide::types::ChatKind::Private(_) => ChatType::Private,
        };

        Chat {
            id: chat.id.0,
            chat_type,
            title: chat.title().map(|s| s.to_string()),
            username: chat.username().map(|s| s.to_string()),
        }
    }

    /// Convert teloxide User to our User type
    fn convert_user(&self, user: &teloxide::types::User) -> User {
        User {
            id: user.id.0 as i64,
            is_bot: user.is_bot,
            first_name: user.first_name.clone(),
            last_name: user.last_name.clone(),
            username: user.username.clone(),
            language_code: user.language_code.clone(),
        }
    }

    /// Publish a text message event
    pub async fn publish_text_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let text = msg.text().unwrap_or_default().to_string();
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        // Update session
        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let event = MessageTextEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            text,
        };

        let subject = subjects::bot::message_text(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published text message event to {}", subject);
        Ok(())
    }

    /// Publish a photo message event
    pub async fn publish_photo_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let photos = msg.photo().unwrap_or_default();
        let caption = msg.caption().map(|s| s.to_string());
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let photo: Vec<PhotoSize> = photos.iter().map(|p| PhotoSize {
            file_id: p.file.id.clone(),
            file_unique_id: p.file.unique_id.clone(),
            width: p.width,
            height: p.height,
            file_size: Some(p.file.size as u64),
        }).collect();

        let event = MessagePhotoEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            photo,
            caption,
        };

        let subject = subjects::bot::message_photo(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published photo message event to {}", subject);
        Ok(())
    }

    /// Publish a video message event
    pub async fn publish_video_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let video = msg.video().unwrap();
        let caption = msg.caption().map(|s| s.to_string());
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let event = MessageVideoEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            video: FileInfo {
                file_id: video.file.id.clone(),
                file_unique_id: video.file.unique_id.clone(),
                file_size: Some(video.file.size as u64),
                file_name: video.file_name.clone(),
                mime_type: video.mime_type.as_ref().map(|m| m.to_string()),
            },
            caption,
        };

        let subject = subjects::bot::message_video(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published video message event to {}", subject);
        Ok(())
    }

    /// Publish an audio message event
    pub async fn publish_audio_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let audio = msg.audio().unwrap();
        let caption = msg.caption().map(|s| s.to_string());
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let event = MessageAudioEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            audio: FileInfo {
                file_id: audio.file.id.clone(),
                file_unique_id: audio.file.unique_id.clone(),
                file_size: Some(audio.file.size as u64),
                file_name: audio.file_name.clone(),
                mime_type: audio.mime_type.as_ref().map(|m| m.to_string()),
            },
            caption,
        };

        let subject = subjects::bot::message_audio(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published audio message event to {}", subject);
        Ok(())
    }

    /// Publish a document message event
    pub async fn publish_document_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let document = msg.document().unwrap();
        let caption = msg.caption().map(|s| s.to_string());
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let event = MessageDocumentEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            document: FileInfo {
                file_id: document.file.id.clone(),
                file_unique_id: document.file.unique_id.clone(),
                file_size: Some(document.file.size as u64),
                file_name: document.file_name.clone(),
                mime_type: document.mime_type.as_ref().map(|m| m.to_string()),
            },
            caption,
        };

        let subject = subjects::bot::message_document(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published document message event to {}", subject);
        Ok(())
    }

    /// Publish a voice message event
    pub async fn publish_voice_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let voice = msg.voice().unwrap();
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let event = MessageVoiceEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            voice: FileInfo {
                file_id: voice.file.id.clone(),
                file_unique_id: voice.file.unique_id.clone(),
                file_size: Some(voice.file.size as u64),
                file_name: None, // Voice messages don't have file names
                mime_type: voice.mime_type.as_ref().map(|m| m.to_string()),
            },
        };

        let subject = subjects::bot::message_voice(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published voice message event to {}", subject);
        Ok(())
    }

    /// Publish a callback query event
    pub async fn publish_callback_query(&self, query: &CallbackQuery, update_id: i64) -> Result<()> {
        let data = query.data.clone().unwrap_or_default();

        // Get chat from message or use a default
        let (chat, message_id) = if let Some(ref msg) = query.message {
            (self.convert_chat(&msg.chat()), Some(msg.id().0))
        } else {
            // For inline queries without a message, we need to handle differently
            warn!("Callback query without message, using user ID as chat");
            let user_chat_id = query.from.id.0 as i64;
            let chat = Chat {
                id: user_chat_id,
                chat_type: ChatType::Private,
                title: None,
                username: None,
            };
            (chat, None)
        };

        let session_id = SessionId::from_chat(&chat);

        let event = CallbackQueryEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            callback_query_id: query.id.clone(),
            from: self.convert_user(&query.from),
            chat,
            message_id,
            data,
        };

        let subject = subjects::bot::callback_query(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published callback query event to {}", subject);
        Ok(())
    }

    /// Publish a command event
    pub async fn publish_command(&self, msg: &Message, command: &str, args: Vec<String>, update_id: i64) -> Result<()> {
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let event = CommandEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            command: command.to_string(),
            args,
        };

        let subject = subjects::bot::command(self.publisher.prefix(), command);
        self.publisher.publish(&subject, &event).await?;

        info!("Published command event: {} to {}", command, subject);
        Ok(())
    }
}
