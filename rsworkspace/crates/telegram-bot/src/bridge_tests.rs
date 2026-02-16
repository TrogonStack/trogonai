#[cfg(test)]
mod tests {
    use telegram_types::chat::{ChatType, Chat};
    use telegram_types::SessionId;

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
}
