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
