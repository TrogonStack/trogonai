#[cfg(test)]
mod tests {
    use crate::bridge::{tg_entity_to_msg_entity, tg_user_to_msg_user};
    use telegram_types::chat::{Chat, ChatType, MessageEntityType};
    use telegram_types::SessionId;
    use teloxide::types::{MessageEntity, MessageEntityKind, UserId};

    fn private(id: i64) -> Chat {
        Chat {
            id,
            chat_type: ChatType::Private,
            title: None,
            username: None,
        }
    }
    fn group(id: i64) -> Chat {
        Chat {
            id,
            chat_type: ChatType::Group,
            title: None,
            username: None,
        }
    }
    fn supergroup(id: i64) -> Chat {
        Chat {
            id,
            chat_type: ChatType::Supergroup,
            title: None,
            username: None,
        }
    }
    fn channel(id: i64) -> Chat {
        Chat {
            id,
            chat_type: ChatType::Channel,
            title: None,
            username: None,
        }
    }

    // ── from_chat() ───────────────────────────────────────────────────────────

    #[test]
    fn test_session_id_from_private_chat() {
        let chat = Chat {
            id: 123456789,
            chat_type: ChatType::Private,
            title: None,
            username: Some("testuser".to_string()),
        };

        let session_id = SessionId::from_chat(&chat);
        assert_eq!(session_id.to_string(), "tg-private-123456789");
    }

    #[test]
    fn test_session_id_from_group_chat() {
        let chat = Chat {
            id: -1001234567890,
            chat_type: ChatType::Group,
            title: Some("Test Group".to_string()),
            username: None,
        };

        let session_id = SessionId::from_chat(&chat);
        assert_eq!(session_id.to_string(), "tg-group--1001234567890");
    }

    #[test]
    fn test_session_id_from_supergroup() {
        let chat = Chat {
            id: -1001234567890,
            chat_type: ChatType::Supergroup,
            title: Some("Test Supergroup".to_string()),
            username: Some("testsupergroup".to_string()),
        };

        let session_id = SessionId::from_chat(&chat);
        assert_eq!(session_id.to_string(), "tg-supergroup--1001234567890");
    }

    #[test]
    fn test_session_id_from_channel() {
        let chat = Chat {
            id: -1001234567890,
            chat_type: ChatType::Channel,
            title: Some("Test Channel".to_string()),
            username: Some("testchannel".to_string()),
        };

        let session_id = SessionId::from_chat(&chat);
        assert_eq!(session_id.to_string(), "tg-channel--1001234567890");
    }

    // ── from_chat_with_user() ─────────────────────────────────────────────────

    #[test]
    fn test_from_chat_with_user_private_always_uses_chat_id() {
        // Private chats: user isolation flag is irrelevant
        let id = SessionId::from_chat_with_user(&private(555), Some(42), true);
        assert_eq!(id.to_string(), "tg-private-555");

        let id2 = SessionId::from_chat_with_user(&private(555), Some(42), false);
        assert_eq!(id2.to_string(), "tg-private-555");
    }

    #[test]
    fn test_from_chat_with_user_group_isolated_with_user() {
        let id = SessionId::from_chat_with_user(&group(100), Some(777), true);
        assert_eq!(id.to_string(), "tg-group-100-777");
    }

    #[test]
    fn test_from_chat_with_user_group_not_isolated() {
        let id = SessionId::from_chat_with_user(&group(100), Some(777), false);
        assert_eq!(id.to_string(), "tg-group-100");
    }

    #[test]
    fn test_from_chat_with_user_group_isolated_no_user() {
        // isolate=true but no user_id → shared group session
        let id = SessionId::from_chat_with_user(&group(100), None, true);
        assert_eq!(id.to_string(), "tg-group-100");
    }

    #[test]
    fn test_from_chat_with_user_supergroup_isolated() {
        let id = SessionId::from_chat_with_user(&supergroup(200), Some(88), true);
        assert_eq!(id.to_string(), "tg-group-200-88");
    }

    #[test]
    fn test_from_chat_with_user_channel_ignores_isolation() {
        let id = SessionId::from_chat_with_user(&channel(300), Some(99), true);
        assert_eq!(id.to_string(), "tg-channel-300");
    }

    // ── tg_entity_to_msg_entity() ─────────────────────────────────────────────

    fn simple_entity(kind: MessageEntityKind) -> MessageEntity {
        MessageEntity { kind, offset: 2, length: 5 }
    }

    #[test]
    fn test_entity_mention() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Mention));
        assert_eq!(e.entity_type, MessageEntityType::Mention);
        assert_eq!(e.offset, 2);
        assert_eq!(e.length, 5);
        assert!(e.url.is_none());
        assert!(e.user.is_none());
        assert!(e.language.is_none());
        assert!(e.custom_emoji_id.is_none());
    }

    #[test]
    fn test_entity_hashtag() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Hashtag));
        assert_eq!(e.entity_type, MessageEntityType::Hashtag);
    }

    #[test]
    fn test_entity_cashtag() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Cashtag));
        assert_eq!(e.entity_type, MessageEntityType::Cashtag);
    }

    #[test]
    fn test_entity_bot_command() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::BotCommand));
        assert_eq!(e.entity_type, MessageEntityType::BotCommand);
    }

    #[test]
    fn test_entity_url() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Url));
        assert_eq!(e.entity_type, MessageEntityType::Url);
    }

    #[test]
    fn test_entity_email() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Email));
        assert_eq!(e.entity_type, MessageEntityType::Email);
    }

    #[test]
    fn test_entity_phone_number() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::PhoneNumber));
        assert_eq!(e.entity_type, MessageEntityType::PhoneNumber);
    }

    #[test]
    fn test_entity_bold() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Bold));
        assert_eq!(e.entity_type, MessageEntityType::Bold);
    }

    #[test]
    fn test_entity_italic() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Italic));
        assert_eq!(e.entity_type, MessageEntityType::Italic);
    }

    #[test]
    fn test_entity_underline() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Underline));
        assert_eq!(e.entity_type, MessageEntityType::Underline);
    }

    #[test]
    fn test_entity_strikethrough() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Strikethrough));
        assert_eq!(e.entity_type, MessageEntityType::Strikethrough);
    }

    #[test]
    fn test_entity_spoiler() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Spoiler));
        assert_eq!(e.entity_type, MessageEntityType::Spoiler);
    }

    #[test]
    fn test_entity_code() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Code));
        assert_eq!(e.entity_type, MessageEntityType::Code);
    }

    #[test]
    fn test_entity_pre_no_language() {
        let kind = MessageEntityKind::Pre { language: None };
        let e = tg_entity_to_msg_entity(&simple_entity(kind));
        assert_eq!(e.entity_type, MessageEntityType::Pre);
        assert!(e.language.is_none());
    }

    #[test]
    fn test_entity_pre_with_language() {
        let kind = MessageEntityKind::Pre { language: Some("rust".to_string()) };
        let e = tg_entity_to_msg_entity(&simple_entity(kind));
        assert_eq!(e.entity_type, MessageEntityType::Pre);
        assert_eq!(e.language.as_deref(), Some("rust"));
    }

    #[test]
    fn test_entity_text_link() {
        let url = url::Url::parse("https://example.com").unwrap();
        let kind = MessageEntityKind::TextLink { url };
        let e = tg_entity_to_msg_entity(&simple_entity(kind));
        assert_eq!(e.entity_type, MessageEntityType::TextLink);
        assert_eq!(e.url.as_deref(), Some("https://example.com/"));
    }

    #[test]
    fn test_entity_text_mention() {
        let user = teloxide::types::User {
            id: UserId(99),
            is_bot: false,
            first_name: "Alice".to_string(),
            last_name: None,
            username: Some("alice".to_string()),
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
        };
        let kind = MessageEntityKind::TextMention { user };
        let e = tg_entity_to_msg_entity(&simple_entity(kind));
        assert_eq!(e.entity_type, MessageEntityType::TextMention);
        let u = e.user.expect("user should be set");
        assert_eq!(u.id, 99);
        assert_eq!(u.first_name, "Alice");
        assert_eq!(u.username.as_deref(), Some("alice"));
    }

    #[test]
    fn test_entity_custom_emoji() {
        let kind = MessageEntityKind::CustomEmoji {
            custom_emoji_id: "emoji_id_abc".to_string(),
        };
        let e = tg_entity_to_msg_entity(&simple_entity(kind));
        assert_eq!(e.entity_type, MessageEntityType::CustomEmoji);
        assert_eq!(e.custom_emoji_id.as_deref(), Some("emoji_id_abc"));
    }

    #[test]
    fn test_entity_blockquote_maps_to_code() {
        let e = tg_entity_to_msg_entity(&simple_entity(MessageEntityKind::Blockquote));
        assert_eq!(e.entity_type, MessageEntityType::Code);
    }

    // ── tg_user_to_msg_user() ────────────────────────────────────────────────

    #[test]
    fn test_tg_user_to_msg_user_full() {
        let user = teloxide::types::User {
            id: UserId(42),
            is_bot: false,
            first_name: "Jorge".to_string(),
            last_name: Some("Garcia".to_string()),
            username: Some("jgarcia".to_string()),
            language_code: Some("es".to_string()),
            is_premium: false,
            added_to_attachment_menu: false,
        };
        let u = tg_user_to_msg_user(&user);
        assert_eq!(u.id, 42);
        assert!(!u.is_bot);
        assert_eq!(u.first_name, "Jorge");
        assert_eq!(u.last_name.as_deref(), Some("Garcia"));
        assert_eq!(u.username.as_deref(), Some("jgarcia"));
        assert_eq!(u.language_code.as_deref(), Some("es"));
    }

    #[test]
    fn test_tg_user_to_msg_user_minimal() {
        let user = teloxide::types::User {
            id: UserId(1),
            is_bot: true,
            first_name: "Bot".to_string(),
            last_name: None,
            username: None,
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
        };
        let u = tg_user_to_msg_user(&user);
        assert_eq!(u.id, 1);
        assert!(u.is_bot);
        assert!(u.last_name.is_none());
        assert!(u.username.is_none());
        assert!(u.language_code.is_none());
    }
}
