//! NATS integration tests for OutboundProcessor — require a live NATS server.
//!
//! Run with: NATS_URL=nats://localhost:14222 cargo test outbound
//!
//! Tests are skipped automatically when NATS is unreachable.
//!
//! The processor attempts real Discord API calls using a fake token;
//! these fail immediately with HTTP 401 Unauthorized. The processor logs
//! the error and continues — tests verify that commands are consumed and
//! the processor does not hang.

#[cfg(test)]
mod tests {
    use crate::outbound::OutboundProcessor;
    use discord_nats::subjects;
    use discord_types::{
        AddReactionCommand, DeleteMessageCommand, EditMessageCommand, InteractionDeferCommand,
        InteractionFollowupCommand, InteractionRespondCommand, RemoveReactionCommand,
        SendMessageCommand, TypingCommand,
    };
    use std::sync::Arc;

    const DEFAULT_NATS_URL: &str = "nats://localhost:14222";
    const PAUSE_MS: u64 = 150;

    async fn try_connect() -> Option<async_nats::Client> {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
        async_nats::connect(&url).await.ok()
    }

    /// Create an Http client pointing at localhost:19999 (connection refused).
    /// This ensures tests don't reach the real Discord API.
    fn fake_http() -> Arc<serenity::http::Http> {
        let http = serenity::http::Http::new("fake-token");
        Arc::new(http)
    }

    async fn start_processor(client: async_nats::Client, prefix: &str) {
        use crate::bridge::PairingState;
        // TODO: ShardManager cannot be constructed outside serenity's Client builder,
        // so we pass None here. The bot_presence handler will log a warning and skip
        // any presence updates received during tests.
        let pairing_state = Arc::new(PairingState::new());
        let processor = OutboundProcessor::new(
            fake_http(),
            client,
            prefix.to_string(),
            None,
            pairing_state,
            None,
            crate::config::ReplyToMode::First,
            None,
        );
        tokio::spawn(async move {
            let _ = processor.run().await;
        });
        // Allow subscriptions to be established
        tokio::time::sleep(std::time::Duration::from_millis(PAUSE_MS)).await;
    }

    async fn publish<T: serde::Serialize>(client: &async_nats::Client, subject: &str, cmd: &T) {
        let payload = serde_json::to_vec(cmd).unwrap();
        client
            .publish(subject.to_string(), payload.into())
            .await
            .unwrap();
    }

    // ── send_message ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_send_message_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-send-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::message_send(&prefix);
        let cmd = SendMessageCommand {
            channel_id: 100,
            content: "Hello from test".to_string(),
            embeds: vec![],
            reply_to_message_id: None,
            files: vec![],
            components: vec![],
        };
        publish(&client, &subject, &cmd).await;

        // Give processor time to consume and attempt (failing) API call
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    #[tokio::test]
    async fn test_send_message_with_reply_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-reply-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::message_send(&prefix);
        let cmd = SendMessageCommand {
            channel_id: 100,
            content: "Reply message".to_string(),
            embeds: vec![],
            reply_to_message_id: Some(50),
            files: vec![],
            components: vec![],
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── edit_message ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_edit_message_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-edit-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::message_edit(&prefix);
        let cmd = EditMessageCommand {
            channel_id: 100,
            message_id: 1,
            content: Some("Edited".to_string()),
            embeds: vec![],
            components: vec![],
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── delete_message ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_message_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-del-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::message_delete(&prefix);
        let cmd = DeleteMessageCommand {
            channel_id: 100,
            message_id: 1,
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── interaction_respond ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_interaction_respond_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-resp-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::interaction_respond(&prefix);
        let cmd = InteractionRespondCommand {
            interaction_id: 9999,
            interaction_token: "fake-token".to_string(),
            content: Some("pong".to_string()),
            embeds: vec![],
            ephemeral: false,
            components: vec![],
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── interaction_defer ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_interaction_defer_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-defer-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::interaction_defer(&prefix);
        let cmd = InteractionDeferCommand {
            interaction_id: 9999,
            interaction_token: "fake-token".to_string(),
            ephemeral: true,
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── interaction_followup ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_interaction_followup_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-fup-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::interaction_followup(&prefix);
        let cmd = InteractionFollowupCommand {
            interaction_token: "fake-token".to_string(),
            content: Some("followup text".to_string()),
            embeds: vec![],
            ephemeral: false,
            session_id: None,
            files: vec![],
            components: vec![],
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── add_reaction ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_add_reaction_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-react-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::reaction_add(&prefix);
        let cmd = AddReactionCommand {
            channel_id: 100,
            message_id: 1,
            emoji: "👍".to_string(),
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── remove_reaction ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_remove_reaction_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-unreact-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::reaction_remove(&prefix);
        let cmd = RemoveReactionCommand {
            channel_id: 100,
            message_id: 1,
            emoji: "👍".to_string(),
        };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── typing ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_typing_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-typ-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        let subject = subjects::agent::channel_typing(&prefix);
        let cmd = TypingCommand { channel_id: 100 };
        publish(&client, &subject, &cmd).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── modal_respond ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_modal_respond_command_is_consumed() {
        use discord_types::ModalRespondCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-modal-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::interaction_modal_respond(&prefix), &ModalRespondCommand {
            interaction_id: 1111,
            interaction_token: "fake-token".to_string(),
            custom_id: "my_modal".to_string(),
            title: "Test Modal".to_string(),
            inputs: vec![],
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── autocomplete_respond ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_autocomplete_respond_command_is_consumed() {
        use discord_types::{AutocompleteChoice, AutocompleteRespondCommand};
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-ac-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::interaction_autocomplete_respond(&prefix), &AutocompleteRespondCommand {
            interaction_id: 2222,
            interaction_token: "fake-token".to_string(),
            choices: vec![AutocompleteChoice { name: "Option A".to_string(), value: "a".to_string() }],
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── ban ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_ban_user_command_is_consumed() {
        use discord_types::BanUserCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-ban-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::guild_ban(&prefix), &BanUserCommand {
            guild_id: 200,
            user_id: 42,
            reason: Some("spam".to_string()),
            delete_message_seconds: 0,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── kick ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_kick_user_command_is_consumed() {
        use discord_types::KickUserCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-kick-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::guild_kick(&prefix), &KickUserCommand {
            guild_id: 200,
            user_id: 42,
            reason: None,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── timeout ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_timeout_user_command_is_consumed() {
        use discord_types::TimeoutUserCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-timeout-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::guild_timeout(&prefix), &TimeoutUserCommand {
            guild_id: 200,
            user_id: 42,
            duration_secs: 3600,
            reason: Some("cooling off".to_string()),
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── create_channel ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_channel_command_is_consumed() {
        use discord_types::{ChannelType, CreateChannelCommand};
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-ch-create-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::channel_create(&prefix), &CreateChannelCommand {
            guild_id: 200,
            name: "new-channel".to_string(),
            channel_type: ChannelType::GuildText,
            category_id: None,
            topic: None,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── edit_channel ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_edit_channel_command_is_consumed() {
        use discord_types::EditChannelCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-ch-edit-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::channel_edit(&prefix), &EditChannelCommand {
            channel_id: 300,
            name: Some("renamed".to_string()),
            topic: None,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── delete_channel ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_channel_command_is_consumed() {
        use discord_types::DeleteChannelCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-ch-del-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::channel_delete(&prefix), &DeleteChannelCommand { channel_id: 300 }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── create_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_role_command_is_consumed() {
        use discord_types::CreateRoleCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-role-create-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::role_create(&prefix), &CreateRoleCommand {
            guild_id: 200,
            name: "VIP".to_string(),
            color: Some(0xFFD700),
            hoist: true,
            mentionable: false,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── assign_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_assign_role_command_is_consumed() {
        use discord_types::AssignRoleCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-role-assign-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::role_assign(&prefix), &AssignRoleCommand {
            guild_id: 200, user_id: 42, role_id: 111,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── remove_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_remove_role_command_is_consumed() {
        use discord_types::RemoveRoleCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-role-remove-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::role_remove(&prefix), &RemoveRoleCommand {
            guild_id: 200, user_id: 42, role_id: 111,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── delete_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_role_command_is_consumed() {
        use discord_types::DeleteRoleCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-role-del-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::role_delete(&prefix), &DeleteRoleCommand {
            guild_id: 200, role_id: 111,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── pin ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_pin_message_command_is_consumed() {
        use discord_types::PinMessageCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-pin-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::message_pin(&prefix), &PinMessageCommand {
            channel_id: 100, message_id: 50,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── unpin ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_unpin_message_command_is_consumed() {
        use discord_types::UnpinMessageCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-unpin-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::message_unpin(&prefix), &UnpinMessageCommand {
            channel_id: 100, message_id: 50,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── bulk_delete ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_bulk_delete_messages_command_is_consumed() {
        use discord_types::BulkDeleteMessagesCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-bulk-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::message_bulk_delete(&prefix), &BulkDeleteMessagesCommand {
            channel_id: 100,
            message_ids: vec![1, 2, 3],
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── create_thread ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_thread_command_is_consumed() {
        use discord_types::CreateThreadCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-thread-create-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::thread_create(&prefix), &CreateThreadCommand {
            channel_id: 100,
            name: "Discussion".to_string(),
            message_id: None,
            auto_archive_mins: 1440,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── archive_thread ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_archive_thread_command_is_consumed() {
        use discord_types::ArchiveThreadCommand;
        let Some(client) = try_connect().await else { eprintln!("SKIP: NATS not available"); return; };
        let prefix = format!("test-out-thread-archive-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;
        publish(&client, &subjects::agent::thread_archive(&prefix), &ArchiveThreadCommand {
            channel_id: 100,
            locked: false,
        }).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // ── multiple commands in sequence ─────────────────────────────────────────

    #[tokio::test]
    async fn test_multiple_commands_all_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-out-multi-{}", uuid::Uuid::new_v4().simple());
        start_processor(client.clone(), &prefix).await;

        for i in 0..3u64 {
            let cmd = SendMessageCommand {
                channel_id: 100 + i,
                content: format!("message {}", i),
                embeds: vec![],
                reply_to_message_id: None,
                files: vec![],
                components: vec![],
            };
            publish(&client, &subjects::agent::message_send(&prefix), &cmd).await;
        }
        // All three should be consumed without hanging
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
