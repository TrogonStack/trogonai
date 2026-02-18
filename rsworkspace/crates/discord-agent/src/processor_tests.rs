//! Unit tests for MessageProcessor

#[cfg(test)]
mod tests {
    use crate::processor::MessageProcessor;

    const NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<async_nats::Client> {
        async_nats::connect(NATS_URL).await.ok()
    }

    /// Build a minimal MessageCreatedEvent for testing.
    fn make_message_event(
        session_id: &str,
        content: &str,
        channel_id: u64,
        message_id: u64,
    ) -> discord_types::events::MessageCreatedEvent {
        use discord_types::events::{EventMetadata, MessageCreatedEvent};
        use discord_types::types::{DiscordMessage, DiscordUser};

        MessageCreatedEvent {
            metadata: EventMetadata::new(session_id, 1),
            message: DiscordMessage {
                id: message_id,
                channel_id,
                guild_id: None,
                author: DiscordUser {
                    id: 42,
                    username: "tester".to_string(),
                    global_name: None,
                    bot: false,
                },
                content: content.to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                edited_timestamp: None,
                attachments: vec![],
                embeds: vec![],
                referenced_message_id: None,
                referenced_message_content: None,
            },
        }
    }

    fn make_slash_command_event(
        session_id: &str,
        command_name: &str,
        channel_id: u64,
    ) -> discord_types::events::SlashCommandEvent {
        use discord_types::events::{EventMetadata, SlashCommandEvent};
        use discord_types::types::DiscordUser;

        SlashCommandEvent {
            metadata: EventMetadata::new(session_id, 1),
            interaction_id: 9999,
            interaction_token: "test-token".to_string(),
            guild_id: None,
            channel_id,
            user: DiscordUser {
                id: 42,
                username: "tester".to_string(),
                global_name: None,
                bot: false,
            },
            command_name: command_name.to_string(),
            options: vec![],
        }
    }

    #[tokio::test]
    async fn test_echo_mode_sends_reply() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-echo-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());
        let subscriber = discord_nats::MessageSubscriber::new(client.clone(), prefix.clone());

        // Subscribe to message.send before processing
        let subject = discord_nats::subjects::agent::message_send(&prefix);
        let mut stream = subscriber
            .subscribe::<discord_types::SendMessageCommand>(&subject)
            .await
            .unwrap();

        let processor = MessageProcessor::default(); // echo mode (no LLM)
        let event = make_message_event("dc-dm-123", "hello world", 100, 50);

        processor.process_message(&event, &publisher).await.unwrap();

        let cmd = stream.next().await.unwrap().unwrap();
        assert_eq!(cmd.channel_id, 100);
        assert!(
            cmd.content.contains("hello world"),
            "echo must include original content"
        );
        assert_eq!(cmd.reply_to_message_id, Some(50));
    }

    #[tokio::test]
    async fn test_echo_mode_skips_empty_message() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-empty-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());

        let processor = MessageProcessor::default();
        let event = make_message_event("dc-dm-123", "", 100, 50);

        // Should not error and should not publish anything
        let result = processor.process_message(&event, &publisher).await;
        assert!(result.is_ok(), "empty message must not return an error");
    }

    #[tokio::test]
    async fn test_ping_slash_command_responds() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-ping-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());
        let subscriber = discord_nats::MessageSubscriber::new(client.clone(), prefix.clone());

        let subject = discord_nats::subjects::agent::interaction_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<discord_types::InteractionRespondCommand>(&subject)
            .await
            .unwrap();

        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-123", "ping", 100);

        processor
            .process_slash_command(&event, &publisher)
            .await
            .unwrap();

        let cmd = stream.next().await.unwrap().unwrap();
        assert_eq!(cmd.interaction_id, 9999);
        assert_eq!(cmd.interaction_token, "test-token");
        let content = cmd.content.unwrap_or_default();
        assert!(
            content.to_lowercase().contains("pong"),
            "ping must reply with pong, got: {}",
            content
        );
    }

    #[tokio::test]
    async fn test_clear_slash_command_clears_session() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-clear-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());
        let subscriber = discord_nats::MessageSubscriber::new(client.clone(), prefix.clone());

        let subject = discord_nats::subjects::agent::interaction_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<discord_types::InteractionRespondCommand>(&subject)
            .await
            .unwrap();

        let processor = MessageProcessor::default();
        let session_id = "dc-dm-999";

        // Seed some conversation history
        processor
            .conversation_manager
            .add_message(session_id, "user", "hello")
            .await;
        assert_eq!(
            processor
                .conversation_manager
                .get_history(session_id)
                .await
                .len(),
            1
        );

        let event = make_slash_command_event(session_id, "clear", 100);
        processor
            .process_slash_command(&event, &publisher)
            .await
            .unwrap();

        // Conversation must be cleared
        assert!(processor
            .conversation_manager
            .get_history(session_id)
            .await
            .is_empty());

        // And we should get an ephemeral response
        let cmd = stream.next().await.unwrap().unwrap();
        assert!(cmd.ephemeral, "clear response must be ephemeral");
    }

    fn make_update_event(
        session_id: &str,
        message_id: u64,
        channel_id: u64,
        new_content: Option<&str>,
    ) -> discord_types::events::MessageUpdatedEvent {
        use discord_types::events::{EventMetadata, MessageUpdatedEvent};
        MessageUpdatedEvent {
            metadata: EventMetadata::new(session_id, 1),
            message_id,
            channel_id,
            guild_id: None,
            new_content: new_content.map(|s| s.to_string()),
            new_embeds: vec![],
        }
    }

    fn make_delete_event(
        session_id: &str,
        message_id: u64,
        channel_id: u64,
    ) -> discord_types::events::MessageDeletedEvent {
        use discord_types::events::{EventMetadata, MessageDeletedEvent};
        MessageDeletedEvent {
            metadata: EventMetadata::new(session_id, 1),
            message_id,
            channel_id,
            guild_id: None,
        }
    }

    fn make_reaction_add_event(
        session_id: &str,
        user_id: u64,
        channel_id: u64,
        message_id: u64,
        emoji: &str,
    ) -> discord_types::events::ReactionAddEvent {
        use discord_types::events::{EventMetadata, ReactionAddEvent};
        use discord_types::types::Emoji;
        ReactionAddEvent {
            metadata: EventMetadata::new(session_id, 1),
            user_id,
            channel_id,
            message_id,
            guild_id: None,
            emoji: Emoji {
                id: None,
                name: emoji.to_string(),
                animated: false,
            },
        }
    }

    // ── process_message_updated ────────────────────────────────────────────

    #[tokio::test]
    async fn test_process_message_update_by_id() {
        let processor = MessageProcessor::default();
        let session_id = "dc-dm-update-1";

        processor
            .conversation_manager
            .add_user_message(session_id, "original", 50)
            .await;

        let event = make_update_event(session_id, 50, 100, Some("edited"));
        processor.process_message_updated(&event).await.unwrap();

        let history = processor
            .conversation_manager
            .get_history(session_id)
            .await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "edited");
    }

    #[tokio::test]
    async fn test_process_message_update_none_content_is_noop() {
        let processor = MessageProcessor::default();
        let session_id = "dc-dm-update-2";

        processor
            .conversation_manager
            .add_user_message(session_id, "original", 50)
            .await;

        let event = make_update_event(session_id, 50, 100, None);
        processor.process_message_updated(&event).await.unwrap();

        let history = processor
            .conversation_manager
            .get_history(session_id)
            .await;
        assert_eq!(history[0].content, "original");
    }

    // ── process_message_deleted ────────────────────────────────────────────

    #[tokio::test]
    async fn test_process_message_delete_removes_turn_and_reply() {
        let processor = MessageProcessor::default();
        let session_id = "dc-dm-delete-1";

        processor
            .conversation_manager
            .add_user_message(session_id, "user msg", 50)
            .await;
        processor
            .conversation_manager
            .add_message(session_id, "assistant", "assistant reply")
            .await;

        let event = make_delete_event(session_id, 50, 100);
        processor.process_message_deleted(&event).await.unwrap();

        let history = processor
            .conversation_manager
            .get_history(session_id)
            .await;
        assert!(
            history.is_empty(),
            "both user msg and assistant reply must be removed"
        );
    }

    #[tokio::test]
    async fn test_process_message_delete_empty_history_is_noop() {
        let processor = MessageProcessor::default();
        let event = make_delete_event("dc-dm-delete-2", 99, 100);
        // Must not panic or error
        processor.process_message_deleted(&event).await.unwrap();
    }

    // ── /forget slash command ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_forget_slash_command_removes_last_exchange() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-forget-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());
        let subscriber = discord_nats::MessageSubscriber::new(client.clone(), prefix.clone());

        let subject = discord_nats::subjects::agent::interaction_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<discord_types::InteractionRespondCommand>(&subject)
            .await
            .unwrap();

        let processor = MessageProcessor::default();
        let session_id = "dc-dm-forget-1";

        processor
            .conversation_manager
            .add_message(session_id, "user", "first question")
            .await;
        processor
            .conversation_manager
            .add_message(session_id, "assistant", "first answer")
            .await;

        let event = make_slash_command_event(session_id, "forget", 100);
        processor
            .process_slash_command(&event, &publisher)
            .await
            .unwrap();

        // Both messages must have been removed
        let history = processor
            .conversation_manager
            .get_history(session_id)
            .await;
        assert!(history.is_empty(), "last exchange must be forgotten");

        // Must get an ephemeral confirmation
        let cmd = stream.next().await.unwrap().unwrap();
        assert!(cmd.ephemeral, "forget response must be ephemeral");
        let content = cmd.content.unwrap_or_default();
        assert!(
            content.contains("removed") || content.contains("forget"),
            "unexpected response: {}",
            content
        );
    }

    #[tokio::test]
    async fn test_forget_slash_command_empty_history() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-forget-empty-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());
        let subscriber = discord_nats::MessageSubscriber::new(client.clone(), prefix.clone());

        let subject = discord_nats::subjects::agent::interaction_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<discord_types::InteractionRespondCommand>(&subject)
            .await
            .unwrap();

        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-forget-2", "forget", 100);

        processor
            .process_slash_command(&event, &publisher)
            .await
            .unwrap();

        let cmd = stream.next().await.unwrap().unwrap();
        assert!(cmd.ephemeral);
        let content = cmd.content.unwrap_or_default();
        assert!(
            content.contains("No conversation history"),
            "unexpected response: {}",
            content
        );
    }

    // ── ❌ reaction clears session ─────────────────────────────────────────

    #[tokio::test]
    async fn test_reaction_clear_clears_session() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-react-clear-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());
        let subscriber = discord_nats::MessageSubscriber::new(client.clone(), prefix.clone());

        let subject = discord_nats::subjects::agent::message_send(&prefix);
        let mut stream = subscriber
            .subscribe::<discord_types::SendMessageCommand>(&subject)
            .await
            .unwrap();

        let processor = MessageProcessor::default();
        // DM reaction → session_id = dc-dm-{channel_id}
        let session_id = "dc-dm-100";
        processor
            .conversation_manager
            .add_message(session_id, "user", "some history")
            .await;

        // user_id=42, channel_id=100, no guild_id → DM session
        let event = make_reaction_add_event(session_id, 42, 100, 77, "❌");
        processor
            .process_reaction_add(&event, &publisher)
            .await
            .unwrap();

        // Session must be cleared
        assert!(
            processor
                .conversation_manager
                .get_history(session_id)
                .await
                .is_empty(),
            "session must be cleared after ❌ reaction"
        );

        // And a confirmation message must be published
        let cmd = stream.next().await.unwrap().unwrap();
        assert_eq!(cmd.channel_id, 100);
        assert!(
            cmd.content.contains("cleared"),
            "confirmation must mention cleared: {}",
            cmd.content
        );
    }

    #[tokio::test]
    async fn test_unknown_slash_command_responds_with_help_hint() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available at {}", NATS_URL);
            return;
        };

        let prefix = format!("test-proc-unknown-{}", uuid::Uuid::new_v4().simple());
        let publisher = discord_nats::MessagePublisher::new(client.clone(), prefix.clone());
        let subscriber = discord_nats::MessageSubscriber::new(client.clone(), prefix.clone());

        let subject = discord_nats::subjects::agent::interaction_respond(&prefix);
        let mut stream = subscriber
            .subscribe::<discord_types::InteractionRespondCommand>(&subject)
            .await
            .unwrap();

        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-123", "nonexistent", 100);

        processor
            .process_slash_command(&event, &publisher)
            .await
            .unwrap();

        let cmd = stream.next().await.unwrap().unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(
            content.contains("/help"),
            "unknown command must mention /help, got: {}",
            content
        );
    }
}
