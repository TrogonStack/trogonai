//! Unit tests for ConversationManager

#[cfg(test)]
mod tests {
    use crate::conversation::ConversationManager;

    #[tokio::test]
    async fn test_new_session_returns_empty_history() {
        let mgr = ConversationManager::new();
        let history = mgr.get_history("session-1").await;
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_add_message_persists_in_memory() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "hello").await;
        let history = mgr.get_history("s1").await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "hello");
    }

    #[tokio::test]
    async fn test_add_multiple_messages_preserves_order() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "first").await;
        mgr.add_message("s1", "assistant", "second").await;
        mgr.add_message("s1", "user", "third").await;
        let history = mgr.get_history("s1").await;
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "first");
        assert_eq!(history[1].content, "second");
        assert_eq!(history[2].content, "third");
    }

    #[tokio::test]
    async fn test_sessions_are_independent() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "from s1").await;
        mgr.add_message("s2", "user", "from s2").await;

        let h1 = mgr.get_history("s1").await;
        let h2 = mgr.get_history("s2").await;

        assert_eq!(h1.len(), 1);
        assert_eq!(h2.len(), 1);
        assert_eq!(h1[0].content, "from s1");
        assert_eq!(h2[0].content, "from s2");
    }

    #[tokio::test]
    async fn test_clear_session_removes_history() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "hello").await;
        mgr.clear_session("s1").await;
        let history = mgr.get_history("s1").await;
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_clear_nonexistent_session_is_noop() {
        let mgr = ConversationManager::new();
        // Should not panic
        mgr.clear_session("nonexistent").await;
        assert_eq!(mgr.active_sessions().await, 0);
    }

    #[tokio::test]
    async fn test_active_sessions_counts_correctly() {
        let mgr = ConversationManager::new();
        assert_eq!(mgr.active_sessions().await, 0);

        mgr.add_message("s1", "user", "hello").await;
        assert_eq!(mgr.active_sessions().await, 1);

        mgr.add_message("s2", "user", "hi").await;
        assert_eq!(mgr.active_sessions().await, 2);

        mgr.clear_session("s1").await;
        assert_eq!(mgr.active_sessions().await, 1);
    }

    #[tokio::test]
    async fn test_history_trimmed_to_max_size() {
        let mgr = ConversationManager::new();
        // MAX_HISTORY_SIZE is 20; add 25 messages
        for i in 0..25u32 {
            mgr.add_message("s1", "user", &format!("msg {}", i)).await;
        }
        let history = mgr.get_history("s1").await;
        // Only the last 20 should remain
        assert_eq!(history.len(), 20);
        // The first message in history should be "msg 5" (25 - 20 = 5)
        assert_eq!(history[0].content, "msg 5");
        assert_eq!(history[19].content, "msg 24");
    }

    #[tokio::test]
    async fn test_add_message_after_clear_starts_fresh() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "old message").await;
        mgr.clear_session("s1").await;
        mgr.add_message("s1", "user", "new message").await;

        let history = mgr.get_history("s1").await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "new message");
    }

    #[tokio::test]
    async fn test_get_history_returns_clone_not_reference() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "user", "hello").await;

        let h1 = mgr.get_history("s1").await;
        let h2 = mgr.get_history("s1").await;

        // Both should have same content independently
        assert_eq!(h1.len(), h2.len());
        assert_eq!(h1[0].content, h2[0].content);
    }

    #[tokio::test]
    async fn test_default_is_same_as_new() {
        let mgr = ConversationManager::default();
        assert_eq!(mgr.active_sessions().await, 0);
        assert!(mgr.get_history("any").await.is_empty());
    }

    #[tokio::test]
    async fn test_add_user_message_stores_discord_id() {
        let mgr = ConversationManager::new();
        mgr.add_user_message("s1", "hello", 999_000_111).await;
        let history = mgr.get_history("s1").await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "hello");
        assert_eq!(history[0].message_id, Some(999_000_111u64));
    }

    #[tokio::test]
    async fn test_add_message_without_id_stores_none() {
        let mgr = ConversationManager::new();
        mgr.add_message("s1", "assistant", "a response").await;
        let history = mgr.get_history("s1").await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].message_id, None);
    }

    #[tokio::test]
    async fn test_with_max_history_custom_limit() {
        let mgr = ConversationManager::with_max_history(5);
        for i in 0..8u32 {
            mgr.add_message("s1", "user", &format!("msg {}", i)).await;
        }
        let history = mgr.get_history("s1").await;
        assert_eq!(history.len(), 5);
        assert_eq!(history[0].content, "msg 3");
        assert_eq!(history[4].content, "msg 7");
    }
}
