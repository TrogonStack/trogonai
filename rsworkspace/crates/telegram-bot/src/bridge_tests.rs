#[cfg(test)]
mod tests {
    use telegram_types::chat::{Chat, ChatType};
    use telegram_types::SessionId;

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
}
