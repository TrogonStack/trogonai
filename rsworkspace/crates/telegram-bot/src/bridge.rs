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
    chat::{Chat, ChatType, User, Message as TgMessage, PhotoSize, FileInfo, ChatMember, ChatMemberStatus, ChatInviteLink, MessageEntity, MessageEntityType},
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
            message_thread_id: msg.thread_id.as_ref().map(|t| t.0.0),
            is_topic_message: Some(msg.is_topic_message),
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

        // Convert entities (mentions, hashtags, etc.)
        let entities = msg.entities().map(|ents| {
            ents.iter()
                .map(|e| self.convert_message_entity(e))
                .collect()
        });

        let event = MessageTextEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            text,
            entities,
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

    /// Publish a sticker message event
    pub async fn publish_sticker_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        use telegram_types::events::{StickerFormat, StickerKind};
        use teloxide::types::StickerKind as TgStickerKind;

        let sticker = msg.sticker().unwrap();
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let format = if sticker.flags.is_video {
            StickerFormat::Video
        } else if sticker.flags.is_animated {
            StickerFormat::Animated
        } else {
            StickerFormat::Static
        };

        let kind = match &sticker.kind {
            TgStickerKind::Regular { .. } => StickerKind::Regular,
            TgStickerKind::Mask { .. } => StickerKind::Mask,
            TgStickerKind::CustomEmoji { .. } => StickerKind::CustomEmoji,
        };

        let thumbnail = sticker.thumbnail.as_ref().map(|t| PhotoSize {
            file_id: t.file.id.clone(),
            file_unique_id: t.file.unique_id.clone(),
            width: t.width,
            height: t.height,
            file_size: Some(t.file.size as u64),
        });

        let event = MessageStickerEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            file_id: sticker.file.id.clone(),
            file_unique_id: sticker.file.unique_id.clone(),
            file_size: Some(sticker.file.size as u64),
            width: sticker.width as u32,
            height: sticker.height as u32,
            format,
            kind,
            emoji: sticker.emoji.clone(),
            set_name: sticker.set_name.clone(),
            thumbnail,
        };

        let subject = subjects::bot::message_sticker(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published sticker message event to {}", subject);
        Ok(())
    }

    /// Publish an animation (GIF) message event
    pub async fn publish_animation_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let animation = msg.animation().unwrap();
        let caption = msg.caption().map(|s| s.to_string());
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let thumbnail = animation.thumbnail.as_ref().map(|t| PhotoSize {
            file_id: t.file.id.clone(),
            file_unique_id: t.file.unique_id.clone(),
            width: t.width,
            height: t.height,
            file_size: Some(t.file.size as u64),
        });

        let event = MessageAnimationEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            animation: FileInfo {
                file_id: animation.file.id.clone(),
                file_unique_id: animation.file.unique_id.clone(),
                file_size: Some(animation.file.size as u64),
                file_name: animation.file_name.clone(),
                mime_type: animation.mime_type.as_ref().map(|m| m.to_string()),
            },
            width: animation.width,
            height: animation.height,
            duration: animation.duration.seconds(),
            thumbnail,
            caption,
        };

        let subject = subjects::bot::message_animation(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published animation message event to {}", subject);
        Ok(())
    }

    /// Publish a video note message event
    pub async fn publish_video_note_message(&self, msg: &Message, update_id: i64) -> Result<()> {
        let video_note = msg.video_note().unwrap();
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let thumbnail = video_note.thumbnail.as_ref().map(|t| PhotoSize {
            file_id: t.file.id.clone(),
            file_unique_id: t.file.unique_id.clone(),
            width: t.width,
            height: t.height,
            file_size: Some(t.file.size as u64),
        });

        let event = MessageVideoNoteEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            video_note: FileInfo {
                file_id: video_note.file.id.clone(),
                file_unique_id: video_note.file.unique_id.clone(),
                file_size: Some(video_note.file.size as u64),
                file_name: None,
                mime_type: None,
            },
            duration: video_note.duration.seconds(),
            length: video_note.length,
            thumbnail,
        };

        let subject = subjects::bot::message_video_note(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        debug!("Published video note message event to {}", subject);
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

    /// Publish an inline query event
    pub async fn publish_inline_query(&self, event: &telegram_types::events::InlineQueryEvent) -> Result<()> {
        let subject = subjects::bot::inline_query(self.publisher.prefix());
        self.publisher.publish(&subject, event).await?;

        debug!("Published inline query event to {}", subject);
        Ok(())
    }

    /// Publish a chosen inline result event
    pub async fn publish_chosen_inline_result(&self, event: &telegram_types::events::ChosenInlineResultEvent) -> Result<()> {
        let subject = subjects::bot::chosen_inline_result(self.publisher.prefix());
        self.publisher.publish(&subject, event).await?;

        debug!("Published chosen inline result event to {}", subject);
        Ok(())
    }

    /// Publish a chat member updated event
    pub async fn publish_chat_member_updated(
        &self,
        update: &teloxide::types::ChatMemberUpdated,
        update_id: i64
    ) -> Result<()> {
        let chat = self.convert_chat(&update.chat);
        let session_id = SessionId::from_chat(&chat);

        let event = ChatMemberUpdatedEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            chat,
            from: self.convert_user(&update.from),
            old_chat_member: self.convert_chat_member(&update.old_chat_member),
            new_chat_member: self.convert_chat_member(&update.new_chat_member),
            date: update.date.timestamp(),
            invite_link: update.invite_link.as_ref().map(|link| self.convert_invite_link(link)),
            via_join_request: Some(update.via_join_request),
            via_chat_folder_invite_link: Some(update.via_chat_folder_invite_link),
        };

        let subject = subjects::bot::chat_member_updated(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        info!("Published chat member updated event to {}", subject);
        Ok(())
    }

    /// Publish a my chat member updated event (bot's status changed)
    pub async fn publish_my_chat_member_updated(
        &self,
        update: &teloxide::types::ChatMemberUpdated,
        update_id: i64
    ) -> Result<()> {
        let chat = self.convert_chat(&update.chat);
        let session_id = SessionId::from_chat(&chat);

        let event = MyChatMemberUpdatedEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            chat,
            from: self.convert_user(&update.from),
            old_chat_member: self.convert_chat_member(&update.old_chat_member),
            new_chat_member: self.convert_chat_member(&update.new_chat_member),
            date: update.date.timestamp(),
            invite_link: update.invite_link.as_ref().map(|link| self.convert_invite_link(link)),
            via_join_request: Some(update.via_join_request),
            via_chat_folder_invite_link: Some(update.via_chat_folder_invite_link),
        };

        let subject = subjects::bot::my_chat_member_updated(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        info!("Published my chat member updated event to {}", subject);
        Ok(())
    }

    /// Publish a pre-checkout query event
    pub async fn publish_pre_checkout_query(
        &self,
        query: &teloxide::types::PreCheckoutQuery,
        update_id: i64,
    ) -> Result<()> {
        // Payments are always in private chats
        let session_id = SessionId::for_private_chat(query.from.id.0 as i64);

        let event = PreCheckoutQueryEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            pre_checkout_query_id: query.id.clone(),
            from: self.convert_user(&query.from),
            currency: query.currency.clone(),
            total_amount: query.total_amount as i64,
            invoice_payload: query.invoice_payload.clone(),
            shipping_option_id: query.shipping_option_id.clone(),
            order_info: Some(self.convert_order_info(&query.order_info)),
        };

        let subject = subjects::bot::payment_pre_checkout(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        info!("Published pre-checkout query event to {}", subject);
        Ok(())
    }

    /// Publish a shipping query event
    pub async fn publish_shipping_query(
        &self,
        query: &teloxide::types::ShippingQuery,
        update_id: i64,
    ) -> Result<()> {
        // Payments are always in private chats
        let session_id = SessionId::for_private_chat(query.from.id.0 as i64);

        let event = ShippingQueryEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            shipping_query_id: query.id.clone(),
            from: self.convert_user(&query.from),
            invoice_payload: query.invoice_payload.clone(),
            shipping_address: self.convert_shipping_address(&query.shipping_address),
        };

        let subject = subjects::bot::payment_shipping(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        info!("Published shipping query event to {}", subject);
        Ok(())
    }

    /// Publish a successful payment event
    pub async fn publish_successful_payment(
        &self,
        msg: &Message,
        payment: &teloxide::types::SuccessfulPayment,
        update_id: i64,
    ) -> Result<()> {
        let chat = self.convert_chat(&msg.chat);
        let session_id = SessionId::from_chat(&chat);

        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64);
        self.session_manager.get_or_create(&session_id, chat.id, user_id).await?;

        let event = SuccessfulPaymentEvent {
            metadata: EventMetadata::new(session_id.to_string(), update_id),
            message: self.convert_message(msg),
            payment: telegram_types::chat::SuccessfulPayment {
                currency: payment.currency.clone(),
                total_amount: payment.total_amount as i64,
                invoice_payload: payment.invoice_payload.clone(),
                telegram_payment_charge_id: payment.telegram_payment_charge_id.clone(),
                provider_payment_charge_id: payment.provider_payment_charge_id.clone(),
                shipping_option_id: payment.shipping_option_id.clone(),
                order_info: Some(self.convert_order_info(&payment.order_info)),
            },
        };

        let subject = subjects::bot::payment_successful(self.publisher.prefix());
        self.publisher.publish(&subject, &event).await?;

        info!("Published successful payment event to {}", subject);
        Ok(())
    }

    /// Convert teloxide ChatMember to our ChatMember type
    fn convert_chat_member(&self, member: &teloxide::types::ChatMember) -> ChatMember {
        use teloxide::types::ChatMemberKind;

        let (status, until_date, is_anonymous, custom_title) = match &member.kind {
            ChatMemberKind::Owner(owner) => (
                ChatMemberStatus::Creator,
                None,
                Some(owner.is_anonymous),
                owner.custom_title.clone(),
            ),
            ChatMemberKind::Administrator(admin) => (
                ChatMemberStatus::Administrator,
                None,
                Some(admin.is_anonymous),
                admin.custom_title.clone(),
            ),
            ChatMemberKind::Member => (
                ChatMemberStatus::Member,
                None,
                None,
                None,
            ),
            ChatMemberKind::Restricted(restricted) => {
                use teloxide::types::UntilDate;
                let until = match restricted.until_date {
                    UntilDate::Date(date) => Some(date.timestamp()),
                    UntilDate::Forever => None,
                };
                (ChatMemberStatus::Restricted, until, None, None)
            },
            ChatMemberKind::Left => (
                ChatMemberStatus::Left,
                None,
                None,
                None,
            ),
            ChatMemberKind::Banned(banned) => {
                use teloxide::types::UntilDate;
                let until = match banned.until_date {
                    UntilDate::Date(date) => Some(date.timestamp()),
                    UntilDate::Forever => None,
                };
                (ChatMemberStatus::Kicked, until, None, None)
            },
        };

        ChatMember {
            user: self.convert_user(&member.user),
            status,
            until_date,
            is_anonymous,
            custom_title,
        }
    }

    /// Convert teloxide ChatInviteLink to our ChatInviteLink type
    fn convert_invite_link(&self, link: &teloxide::types::ChatInviteLink) -> ChatInviteLink {
        ChatInviteLink {
            invite_link: link.invite_link.clone(),
            creator: self.convert_user(&link.creator),
            name: link.name.clone(),
            creates_join_request: link.creates_join_request,
            is_primary: link.is_primary,
            is_revoked: link.is_revoked,
            expire_date: link.expire_date.map(|d| d.timestamp()),
            member_limit: link.member_limit.map(|l| l as i32),
        }
    }

    /// Convert teloxide MessageEntity to our MessageEntity type
    fn convert_message_entity(&self, entity: &teloxide::types::MessageEntity) -> MessageEntity {
        use teloxide::types::MessageEntityKind;

        let entity_type = match &entity.kind {
            MessageEntityKind::Mention => MessageEntityType::Mention,
            MessageEntityKind::Hashtag => MessageEntityType::Hashtag,
            MessageEntityKind::Cashtag => MessageEntityType::Cashtag,
            MessageEntityKind::BotCommand => MessageEntityType::BotCommand,
            MessageEntityKind::Url => MessageEntityType::Url,
            MessageEntityKind::Email => MessageEntityType::Email,
            MessageEntityKind::PhoneNumber => MessageEntityType::PhoneNumber,
            MessageEntityKind::Bold => MessageEntityType::Bold,
            MessageEntityKind::Italic => MessageEntityType::Italic,
            MessageEntityKind::Underline => MessageEntityType::Underline,
            MessageEntityKind::Strikethrough => MessageEntityType::Strikethrough,
            MessageEntityKind::Spoiler => MessageEntityType::Spoiler,
            MessageEntityKind::Code => MessageEntityType::Code,
            MessageEntityKind::Pre { language: _ } => MessageEntityType::Pre,
            MessageEntityKind::TextLink { url: _ } => MessageEntityType::TextLink,
            MessageEntityKind::TextMention { user: _ } => MessageEntityType::TextMention,
            MessageEntityKind::CustomEmoji { custom_emoji_id: _ } => MessageEntityType::CustomEmoji,
            MessageEntityKind::Blockquote => MessageEntityType::Code, // Map blockquote to code for now
            MessageEntityKind::ExpandableBlockquote => MessageEntityType::Code, // Map expandable blockquote to code for now
        };

        MessageEntity {
            entity_type,
            offset: entity.offset as i32,
            length: entity.length as i32,
            url: match &entity.kind {
                MessageEntityKind::TextLink { url } => Some(url.to_string()),
                _ => None,
            },
            user: match &entity.kind {
                MessageEntityKind::TextMention { user } => Some(self.convert_user(user)),
                _ => None,
            },
            language: match &entity.kind {
                MessageEntityKind::Pre { language } => language.clone(),
                _ => None,
            },
            custom_emoji_id: match &entity.kind {
                MessageEntityKind::CustomEmoji { custom_emoji_id } => Some(custom_emoji_id.clone()),
                _ => None,
            },
        }
    }

    /// Convert teloxide OrderInfo to our OrderInfo type
    fn convert_order_info(&self, info: &teloxide::types::OrderInfo) -> telegram_types::chat::OrderInfo {
        telegram_types::chat::OrderInfo {
            name: info.name.clone(),
            phone_number: info.phone_number.clone(),
            email: info.email.clone(),
            shipping_address: info.shipping_address.as_ref().map(|addr| self.convert_shipping_address(addr)),
        }
    }

    /// Convert teloxide ShippingAddress to our ShippingAddress type
    fn convert_shipping_address(&self, addr: &teloxide::types::ShippingAddress) -> telegram_types::chat::ShippingAddress {
        // CountryCode is an enum, use serde_json to get ISO 3166-1 alpha-2 string
        let country_code = serde_json::to_string(&addr.country_code)
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();

        telegram_types::chat::ShippingAddress {
            country_code,
            state: addr.state.clone(),
            city: addr.city.clone(),
            street_line1: addr.street_line1.clone(),
            street_line2: addr.street_line2.clone(),
            post_code: addr.post_code.clone(),
        }
    }
}
