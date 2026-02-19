//! Unit tests for MessageProcessor

#[cfg(test)]
mod tests {
    use crate::processor::MessageProcessor;
    use discord_nats::MockPublisher;

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
            pluralkit_member_id: None,
            pluralkit_member_name: None,
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
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default(); // echo mode (no LLM)
        let event = make_message_event("dc-dm-123", "hello world", 100, 50);

        processor.process_message(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        // send_typing + message_send
        assert_eq!(messages.len(), 2);
        let cmd: discord_types::SendMessageCommand =
            serde_json::from_value(messages[1].1.clone()).unwrap();
        assert_eq!(cmd.channel_id, 100);
        assert!(
            cmd.content.contains("hello world"),
            "echo must include original content"
        );
        assert_eq!(cmd.reply_to_message_id, Some(50));
    }

    #[tokio::test]
    async fn test_echo_mode_skips_empty_message() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_message_event("dc-dm-123", "", 100, 50);

        let result = processor.process_message(&event, &mock).await;
        assert!(result.is_ok(), "empty message must not return an error");
        assert!(mock.is_empty(), "empty message must not publish anything");
    }

    #[tokio::test]
    async fn test_ping_slash_command_responds() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-123", "ping", 100);

        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
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
        let mock = MockPublisher::new("test");
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
            .process_slash_command(&event, &mock)
            .await
            .unwrap();

        // Conversation must be cleared
        assert!(processor
            .conversation_manager
            .get_history(session_id)
            .await
            .is_empty());

        // And we should get an ephemeral response
        let messages = mock.published_messages();
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
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
        let mock = MockPublisher::new("test");
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
            .process_slash_command(&event, &mock)
            .await
            .unwrap();

        // Both messages must have been removed
        let history = processor
            .conversation_manager
            .get_history(session_id)
            .await;
        assert!(history.is_empty(), "last exchange must be forgotten");

        // Must get an ephemeral confirmation
        let messages = mock.published_messages();
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
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
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-forget-2", "forget", 100);

        processor
            .process_slash_command(&event, &mock)
            .await
            .unwrap();

        let messages = mock.published_messages();
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
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
        let mock = MockPublisher::new("test");
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
            .process_reaction_add(&event, &mock)
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
        let messages = mock.published_messages();
        let cmd: discord_types::SendMessageCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert_eq!(cmd.channel_id, 100);
        assert!(
            cmd.content.contains("cleared"),
            "confirmation must mention cleared: {}",
            cmd.content
        );
    }

    // ── lifecycle event stubs (no publisher needed) ────────────────────────

    fn make_typing_event(
        session_id: &str,
        user_id: u64,
        channel_id: u64,
    ) -> discord_types::events::TypingStartEvent {
        use discord_types::events::{EventMetadata, TypingStartEvent};
        TypingStartEvent {
            metadata: EventMetadata::new(session_id, 1),
            user_id,
            channel_id,
            guild_id: None,
        }
    }

    fn make_voice_event(
        session_id: &str,
        user_id: u64,
    ) -> discord_types::events::VoiceStateUpdateEvent {
        use discord_types::events::{EventMetadata, VoiceStateUpdateEvent};
        use discord_types::types::VoiceState;
        VoiceStateUpdateEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild_id: Some(200),
            old_channel_id: None,
            new_state: VoiceState {
                user_id,
                channel_id: Some(500),
                guild_id: Some(200),
                self_mute: false,
                self_deaf: false,
            },
        }
    }

    fn make_guild_create_event(session_id: &str) -> discord_types::events::GuildCreateEvent {
        use discord_types::events::{EventMetadata, GuildCreateEvent};
        use discord_types::types::DiscordGuild;
        GuildCreateEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild: DiscordGuild {
                id: 200,
                name: "Test".to_string(),
            },
            member_count: 10,
        }
    }

    fn make_guild_update_event(session_id: &str) -> discord_types::events::GuildUpdateEvent {
        use discord_types::events::{EventMetadata, GuildUpdateEvent};
        use discord_types::types::DiscordGuild;
        GuildUpdateEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild: DiscordGuild {
                id: 200,
                name: "Updated".to_string(),
            },
        }
    }

    fn make_guild_delete_event(session_id: &str) -> discord_types::events::GuildDeleteEvent {
        use discord_types::events::{EventMetadata, GuildDeleteEvent};
        GuildDeleteEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild_id: 200,
            unavailable: false,
        }
    }

    fn make_channel_create_event(session_id: &str) -> discord_types::events::ChannelCreateEvent {
        use discord_types::events::{EventMetadata, ChannelCreateEvent};
        use discord_types::types::{ChannelType, DiscordChannel};
        ChannelCreateEvent {
            metadata: EventMetadata::new(session_id, 1),
            channel: DiscordChannel {
                id: 300,
                channel_type: ChannelType::GuildText,
                guild_id: Some(200),
                name: Some("general".to_string()),
            },
        }
    }

    fn make_channel_update_event(session_id: &str) -> discord_types::events::ChannelUpdateEvent {
        use discord_types::events::{EventMetadata, ChannelUpdateEvent};
        use discord_types::types::{ChannelType, DiscordChannel};
        ChannelUpdateEvent {
            metadata: EventMetadata::new(session_id, 1),
            channel: DiscordChannel {
                id: 300,
                channel_type: ChannelType::GuildText,
                guild_id: Some(200),
                name: Some("renamed".to_string()),
            },
        }
    }

    fn make_channel_delete_event(session_id: &str) -> discord_types::events::ChannelDeleteEvent {
        use discord_types::events::{EventMetadata, ChannelDeleteEvent};
        ChannelDeleteEvent {
            metadata: EventMetadata::new(session_id, 1),
            channel_id: 300,
            guild_id: 200,
        }
    }

    fn make_role_create_event(session_id: &str) -> discord_types::events::RoleCreateEvent {
        use discord_types::events::{EventMetadata, RoleCreateEvent};
        use discord_types::types::DiscordRole;
        RoleCreateEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild_id: 200,
            role: DiscordRole {
                id: 111,
                name: "Mod".to_string(),
                color: 0,
                hoist: false,
                position: 1,
                permissions: "0".to_string(),
                mentionable: false,
            },
        }
    }

    fn make_role_update_event(session_id: &str) -> discord_types::events::RoleUpdateEvent {
        use discord_types::events::{EventMetadata, RoleUpdateEvent};
        use discord_types::types::DiscordRole;
        RoleUpdateEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild_id: 200,
            role: DiscordRole {
                id: 111,
                name: "Admin".to_string(),
                color: 0xFF0000,
                hoist: true,
                position: 1,
                permissions: "8".to_string(),
                mentionable: true,
            },
        }
    }

    fn make_role_delete_event(session_id: &str) -> discord_types::events::RoleDeleteEvent {
        use discord_types::events::{EventMetadata, RoleDeleteEvent};
        RoleDeleteEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild_id: 200,
            role_id: 111,
        }
    }

    fn make_presence_event(session_id: &str) -> discord_types::events::PresenceUpdateEvent {
        use discord_types::events::{EventMetadata, PresenceUpdateEvent};
        PresenceUpdateEvent {
            metadata: EventMetadata::new(session_id, 1),
            user_id: 42,
            guild_id: 200,
            status: "online".to_string(),
        }
    }

    fn make_bot_ready_event(session_id: &str) -> discord_types::events::BotReadyEvent {
        use discord_types::events::{EventMetadata, BotReadyEvent};
        use discord_types::types::DiscordUser;
        BotReadyEvent {
            metadata: EventMetadata::new(session_id, 1),
            bot_user: DiscordUser {
                id: 999,
                username: "bot".to_string(),
                global_name: None,
                bot: true,
            },
            guild_count: 3,
        }
    }

    fn make_autocomplete_event(
        session_id: &str,
        interaction_id: u64,
    ) -> discord_types::events::AutocompleteEvent {
        use discord_types::events::{AutocompleteEvent, EventMetadata};
        use discord_types::types::DiscordUser;
        AutocompleteEvent {
            metadata: EventMetadata::new(session_id, 1),
            interaction_id,
            interaction_token: "ac-tok".to_string(),
            guild_id: None,
            channel_id: 100,
            user: DiscordUser {
                id: 42,
                username: "tester".to_string(),
                global_name: None,
                bot: false,
            },
            command_name: "ask".to_string(),
            focused_option: "question".to_string(),
            current_value: "how".to_string(),
        }
    }

    fn make_modal_submit_event(
        session_id: &str,
        interaction_id: u64,
    ) -> discord_types::events::ModalSubmitEvent {
        use discord_types::events::{EventMetadata, ModalSubmitEvent};
        use discord_types::types::DiscordUser;
        ModalSubmitEvent {
            metadata: EventMetadata::new(session_id, 1),
            interaction_id,
            interaction_token: "modal-tok".to_string(),
            guild_id: None,
            channel_id: 100,
            user: DiscordUser {
                id: 42,
                username: "tester".to_string(),
                global_name: None,
                bot: false,
            },
            custom_id: "my_modal".to_string(),
            inputs: vec![],
        }
    }

    #[tokio::test]
    async fn test_typing_start_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_typing_start(&make_typing_event("dc-dm-100", 42, 100))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_voice_state_update_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_voice_state_update(&make_voice_event("dc-guild-200-500", 42))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_guild_create_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_guild_create(&make_guild_create_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_guild_update_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_guild_update(&make_guild_update_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_guild_delete_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_guild_delete(&make_guild_delete_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_channel_create_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_channel_create(&make_channel_create_event("dc-guild-200-300"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_channel_update_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_channel_update(&make_channel_update_event("dc-guild-200-300"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_channel_delete_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_channel_delete(&make_channel_delete_event("dc-guild-200-300"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_role_create_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_role_create(&make_role_create_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_role_update_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_role_update(&make_role_update_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_role_delete_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_role_delete(&make_role_delete_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_presence_update_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_presence_update(&make_presence_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_bot_ready_is_noop() {
        let processor = MessageProcessor::default();
        let result = processor
            .process_bot_ready(&make_bot_ready_event("dc-guild-200-100"))
            .await;
        assert!(result.is_ok());
    }

    // ── autocomplete & modal ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_autocomplete_responds_with_typed_value() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_autocomplete_event("dc-dm-100", 6666);

        processor.process_autocomplete(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert_eq!(messages.len(), 1);
        let cmd: discord_types::AutocompleteRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert_eq!(cmd.interaction_id, 6666);
        assert_eq!(cmd.interaction_token, "ac-tok");
        assert_eq!(
            cmd.choices.len(),
            1,
            "non-empty current_value must return one pass-through choice"
        );
        assert_eq!(cmd.choices[0].name, "how");
        assert_eq!(cmd.choices[0].value, "how");
    }

    #[tokio::test]
    async fn test_autocomplete_empty_value_returns_no_choices() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = {
            use discord_types::events::{AutocompleteEvent, EventMetadata};
            use discord_types::types::DiscordUser;
            AutocompleteEvent {
                metadata: EventMetadata::new("dc-dm-100", 1),
                interaction_id: 6667,
                interaction_token: "ac-tok-empty".to_string(),
                guild_id: None,
                channel_id: 100,
                user: DiscordUser {
                    id: 42,
                    username: "tester".to_string(),
                    global_name: None,
                    bot: false,
                },
                command_name: "ask".to_string(),
                focused_option: "question".to_string(),
                current_value: String::new(),
            }
        };

        processor.process_autocomplete(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert_eq!(messages.len(), 1);
        let cmd: discord_types::AutocompleteRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert_eq!(cmd.interaction_id, 6667);
        assert!(
            cmd.choices.is_empty(),
            "empty current_value must return no choices"
        );
    }

    #[tokio::test]
    async fn test_modal_submit_responds_ephemeral() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_modal_submit_event("dc-dm-100", 7777);

        processor.process_modal_submit(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert_eq!(cmd.interaction_id, 7777);
        assert_eq!(cmd.interaction_token, "modal-tok");
        assert!(cmd.ephemeral, "modal ack must be ephemeral");
        let content = cmd.content.unwrap_or_default();
        assert!(
            content.contains("Received"),
            "modal ack content must say Received, got: {}",
            content
        );
    }

    #[tokio::test]
    async fn test_unknown_slash_command_responds_with_help_hint() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-123", "nonexistent", 100);

        processor
            .process_slash_command(&event, &mock)
            .await
            .unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(
            content.contains("/help"),
            "unknown command must mention /help, got: {}",
            content
        );
    }

    // ── /help ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_help_slash_command_echo_mode() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-123", "help", 100);

        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(content.contains("echo mode"), "echo mode help must mention echo mode, got: {}", content);
        assert!(!cmd.ephemeral, "/help must not be ephemeral");
    }

    // ── /status ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_status_slash_command_echo_mode() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-123", "status", 100);

        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(content.contains("Echo mode"), "status must show mode, got: {}", content);
        assert!(content.contains("Ready"), "status must show Ready");
    }

    // ── /ask ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_ask_slash_command_echo_mode() {
        use discord_types::types::{CommandOption, CommandOptionValue};
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();

        let mut event = make_slash_command_event("dc-dm-123", "ask", 100);
        event.options.push(CommandOption {
            name: "question".to_string(),
            value: CommandOptionValue::String("What is 2+2?".to_string()),
        });

        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(content.contains("What is 2+2?"), "echo /ask must include question, got: {}", content);
    }

    #[tokio::test]
    async fn test_ask_slash_command_empty_question() {
        use discord_types::types::{CommandOption, CommandOptionValue};
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();

        let mut event = make_slash_command_event("dc-dm-123", "ask", 100);
        event.options.push(CommandOption {
            name: "question".to_string(),
            value: CommandOptionValue::String(String::new()),
        });

        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert!(cmd.ephemeral, "empty question must be ephemeral");
        let content = cmd.content.unwrap_or_default();
        assert!(content.contains("question"), "must prompt for question, got: {}", content);
    }

    #[tokio::test]
    async fn test_ask_slash_command_no_options() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        // No options at all — question defaults to empty
        let event = make_slash_command_event("dc-dm-123", "ask", 100);

        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert!(cmd.ephemeral);
    }

    // ── /summarize ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_summarize_requires_llm_in_echo_mode() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        // Add some history so it doesn't hit "empty history" first
        let session_id = "dc-dm-sum-1";
        processor.conversation_manager.add_message(session_id, "user", "hi").await;

        let event = make_slash_command_event(session_id, "summarize", 100);
        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(
            content.contains("LLM mode"),
            "summarize without LLM must say LLM mode required, got: {}",
            content
        );
        assert!(cmd.ephemeral);
    }

    #[tokio::test]
    async fn test_summarize_empty_history_responds_ephemeral() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_slash_command_event("dc-dm-sum-empty", "summarize", 100);

        processor.process_slash_command(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(content.contains("No conversation history"), "got: {}", content);
        assert!(cmd.ephemeral);
    }

    // ── welcome / farewell (member add / remove) ───────────────────────────

    fn make_member_add_event(
        session_id: &str,
        user_id: u64,
        username: &str,
        guild_id: u64,
        nick: Option<&str>,
        global_name: Option<&str>,
    ) -> discord_types::events::GuildMemberAddEvent {
        use discord_types::events::{EventMetadata, GuildMemberAddEvent};
        use discord_types::types::{DiscordMember, DiscordUser};
        GuildMemberAddEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild_id,
            member: DiscordMember {
                user: DiscordUser {
                    id: user_id,
                    username: username.to_string(),
                    global_name: global_name.map(|s| s.to_string()),
                    bot: false,
                },
                guild_id,
                nick: nick.map(|s| s.to_string()),
                roles: vec![],
            },
        }
    }

    fn make_member_remove_event(
        session_id: &str,
        user_id: u64,
        username: &str,
        guild_id: u64,
        global_name: Option<&str>,
    ) -> discord_types::events::GuildMemberRemoveEvent {
        use discord_types::events::{EventMetadata, GuildMemberRemoveEvent};
        use discord_types::types::DiscordUser;
        GuildMemberRemoveEvent {
            metadata: EventMetadata::new(session_id, 1),
            guild_id,
            user: DiscordUser {
                id: user_id,
                username: username.to_string(),
                global_name: global_name.map(|s| s.to_string()),
                bot: false,
            },
        }
    }

    #[tokio::test]
    async fn test_member_add_no_welcome_config_is_noop() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default(); // no welcome config
        let event = make_member_add_event("dc-guild-200-100", 42, "alice", 200, None, None);

        processor.process_member_add(&event, &mock).await.unwrap();
        assert!(mock.is_empty(), "no welcome config must publish nothing");
    }

    #[tokio::test]
    async fn test_member_add_sends_welcome_with_mention() {
        use crate::processor::WelcomeConfig;
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::new(
            None, None, None,
            Some(WelcomeConfig { channel_id: 500, template: "Welcome {user}!".to_string() }),
            None, None, None, 20,
            tokio::time::Duration::from_secs(120), None,
        );
        let event = make_member_add_event("dc-guild-200-100", 42, "alice", 200, None, None);

        processor.process_member_add(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert_eq!(messages.len(), 1);
        let cmd: discord_types::SendMessageCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert_eq!(cmd.channel_id, 500);
        assert!(cmd.content.contains("<@42>"), "must mention user, got: {}", cmd.content);
    }

    #[tokio::test]
    async fn test_member_add_uses_nick_when_present() {
        use crate::processor::WelcomeConfig;
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::new(
            None, None, None,
            Some(WelcomeConfig { channel_id: 500, template: "Hey {username}!".to_string() }),
            None, None, None, 20,
            tokio::time::Duration::from_secs(120), None,
        );
        let event = make_member_add_event("dc-guild-200-100", 42, "alice", 200, Some("Ally"), None);

        processor.process_member_add(&event, &mock).await.unwrap();

        let cmd: discord_types::SendMessageCommand =
            serde_json::from_value(mock.published_messages()[0].1.clone()).unwrap();
        assert!(cmd.content.contains("Ally"), "must use nick, got: {}", cmd.content);
    }

    #[tokio::test]
    async fn test_member_add_uses_global_name_without_nick() {
        use crate::processor::WelcomeConfig;
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::new(
            None, None, None,
            Some(WelcomeConfig { channel_id: 500, template: "Hey {username}!".to_string() }),
            None, None, None, 20,
            tokio::time::Duration::from_secs(120), None,
        );
        let event = make_member_add_event("dc-guild-200-100", 42, "alice", 200, None, Some("Alice Global"));

        processor.process_member_add(&event, &mock).await.unwrap();

        let cmd: discord_types::SendMessageCommand =
            serde_json::from_value(mock.published_messages()[0].1.clone()).unwrap();
        assert!(cmd.content.contains("Alice Global"), "must use global_name, got: {}", cmd.content);
    }

    #[tokio::test]
    async fn test_member_remove_no_farewell_config_is_noop() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_member_remove_event("dc-guild-200-100", 42, "alice", 200, None);

        processor.process_member_remove(&event, &mock).await.unwrap();
        assert!(mock.is_empty(), "no farewell config must publish nothing");
    }

    #[tokio::test]
    async fn test_member_remove_sends_farewell() {
        use crate::processor::WelcomeConfig;
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::new(
            None, None, None, None,
            Some(WelcomeConfig { channel_id: 600, template: "Goodbye {username}!".to_string() }),
            None, None, 20,
            tokio::time::Duration::from_secs(120), None,
        );
        let event = make_member_remove_event("dc-guild-200-100", 42, "alice", 200, Some("Alice"));

        processor.process_member_remove(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert_eq!(messages.len(), 1);
        let cmd: discord_types::SendMessageCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert_eq!(cmd.channel_id, 600);
        assert!(cmd.content.contains("Alice"), "must use display name, got: {}", cmd.content);
    }

    // ── reaction_add: edge cases ───────────────────────────────────────────

    #[tokio::test]
    async fn test_reaction_add_unknown_emoji_is_noop() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_reaction_add_event("dc-dm-100", 42, 100, 77, "👍");

        processor.process_reaction_add(&event, &mock).await.unwrap();
        assert!(mock.is_empty(), "unknown emoji must publish nothing");
    }

    #[tokio::test]
    async fn test_reaction_add_regenerate_echo_mode_is_noop() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default(); // echo mode = no LLM
        let event = make_reaction_add_event("dc-dm-100", 42, 100, 77, "🔁");

        processor.process_reaction_add(&event, &mock).await.unwrap();
        assert!(mock.is_empty(), "🔁 in echo mode must publish nothing");
    }

    #[tokio::test]
    async fn test_reaction_add_rotate_echo_mode_is_noop() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_reaction_add_event("dc-dm-100", 42, 100, 77, "🔄");

        processor.process_reaction_add(&event, &mock).await.unwrap();
        assert!(mock.is_empty(), "🔄 in echo mode must publish nothing");
    }

    // ── reaction_remove ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_reaction_remove_is_noop() {
        use discord_types::events::{EventMetadata, ReactionRemoveEvent};
        use discord_types::types::Emoji;
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = ReactionRemoveEvent {
            metadata: EventMetadata::new("dc-dm-100", 1),
            user_id: 42,
            channel_id: 100,
            message_id: 77,
            guild_id: None,
            emoji: Emoji { id: None, name: "👍".to_string(), animated: false },
        };
        processor.process_reaction_remove(&event, &mock).await.unwrap();
        assert!(mock.is_empty());
    }

    // ── component_interaction (echo mode) ─────────────────────────────────

    fn make_component_event(
        session_id: &str,
        custom_id: &str,
        values: Vec<String>,
    ) -> discord_types::events::ComponentInteractionEvent {
        use discord_types::events::{ComponentInteractionEvent, EventMetadata};
        use discord_types::types::{ComponentType, DiscordUser};
        ComponentInteractionEvent {
            metadata: EventMetadata::new(session_id, 1),
            interaction_id: 8888,
            interaction_token: "comp-tok".to_string(),
            guild_id: None,
            channel_id: 100,
            user: DiscordUser { id: 42, username: "tester".to_string(), global_name: None, bot: false },
            message_id: 77,
            custom_id: custom_id.to_string(),
            component_type: if values.is_empty() { ComponentType::Button } else { ComponentType::StringSelect },
            values,
        }
    }

    #[tokio::test]
    async fn test_component_button_click_echo_mode() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_component_event("dc-dm-100", "my_button", vec![]);

        processor.process_component_interaction(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert!(cmd.ephemeral, "component echo must be ephemeral");
        let content = cmd.content.unwrap_or_default();
        assert!(content.contains("my_button"), "must include custom_id, got: {}", content);
    }

    #[tokio::test]
    async fn test_component_select_menu_echo_mode() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default();
        let event = make_component_event(
            "dc-dm-100", "my_select",
            vec!["option_a".to_string(), "option_b".to_string()],
        );

        processor.process_component_interaction(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        assert!(!messages.is_empty());
        let cmd: discord_types::InteractionRespondCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        let content = cmd.content.unwrap_or_default();
        assert!(content.contains("option_a"), "must include selected values, got: {}", content);
    }

    // ── process_message_deleted: heuristic and legacy paths ────────────────

    #[tokio::test]
    async fn test_process_message_delete_heuristic_removes_last_pair() {
        // Step 2: last pair is user+assistant (no ID match) → remove both
        let processor = MessageProcessor::default();
        let session_id = "dc-dm-del-heuristic";

        // Add user msg without message_id (simulates old history without ID tracking)
        processor.conversation_manager.add_message(session_id, "user", "old user msg").await;
        processor.conversation_manager.add_message(session_id, "assistant", "old assistant reply").await;

        // Delete event with an ID that doesn't match (no message stored with this ID)
        let event = make_delete_event(session_id, 9999, 100);
        processor.process_message_deleted(&event).await.unwrap();

        let history = processor.conversation_manager.get_history(session_id).await;
        assert!(history.is_empty(), "heuristic step must remove last user+assistant pair");
    }

    #[tokio::test]
    async fn test_process_message_delete_legacy_fallback_user_only() {
        // Step 3: only a user message remains (no assistant after it)
        let processor = MessageProcessor::default();
        let session_id = "dc-dm-del-legacy";

        processor.conversation_manager.add_message(session_id, "user", "lone user msg").await;

        let event = make_delete_event(session_id, 9999, 100);
        processor.process_message_deleted(&event).await.unwrap();

        let history = processor.conversation_manager.get_history(session_id).await;
        assert!(history.is_empty(), "legacy fallback must remove last user message");
    }

    // ── ack_emoji ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_ack_emoji_published_before_typing() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::new(
            None, None, None, None, None, None, None, 20,
            tokio::time::Duration::from_secs(120),
            Some("⏳".to_string()),
        );
        let event = make_message_event("dc-dm-123", "hello", 100, 50);

        processor.process_message(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        // ack_reaction + typing + message_send = 3
        assert!(messages.len() >= 3, "must have ack reaction, typing, and send message");
        // First publish must be the reaction
        let reaction: discord_types::AddReactionCommand =
            serde_json::from_value(messages[0].1.clone()).unwrap();
        assert_eq!(reaction.emoji, "⏳");
        assert_eq!(reaction.message_id, 50);
    }

    // ── guild_member_update ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_guild_member_update_is_noop() {
        use discord_types::events::{EventMetadata, GuildMemberUpdateEvent};
        use discord_types::types::DiscordUser;
        let processor = MessageProcessor::default();
        let event = GuildMemberUpdateEvent {
            metadata: EventMetadata::new("dc-guild-200-100", 1),
            guild_id: 200,
            user: DiscordUser { id: 42, username: "alice".to_string(), global_name: None, bot: false },
            nick: Some("Ally".to_string()),
            roles: vec![111],
        };
        // Must not panic
        processor.process_guild_member_update(&event).await;
    }

    // ── message with ack_emoji = None (default) ────────────────────────────

    #[tokio::test]
    async fn test_no_ack_emoji_only_typing_and_send() {
        let mock = MockPublisher::new("test");
        let processor = MessageProcessor::default(); // ack_emoji = None
        let event = make_message_event("dc-dm-456", "test msg", 200, 99);

        processor.process_message(&event, &mock).await.unwrap();

        let messages = mock.published_messages();
        // typing + message_send = 2 (no reaction)
        assert_eq!(messages.len(), 2, "without ack_emoji must publish exactly 2 messages");
    }
}
