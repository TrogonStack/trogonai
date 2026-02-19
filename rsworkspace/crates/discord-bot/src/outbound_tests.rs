//! Outbound processor integration tests with real HTTP assertions.
//!
//! Uses wiremock to intercept Discord API calls via serenity's `HttpBuilder::proxy()`.
//! Each test verifies the exact HTTP method, path, and body sent to Discord.
//!
//! Run with: NATS_URL=nats://localhost:14222 cargo test outbound
//! Tests skip automatically when NATS is unreachable.

#[cfg(test)]
mod tests {
    use crate::outbound::OutboundProcessor;
    use discord_nats::subjects;
    use discord_types::*;
    use std::sync::Arc;
    use wiremock::matchers::{body_string_contains, method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const DEFAULT_NATS_URL: &str = "nats://localhost:14222";
    const PAUSE_MS: u64 = 150;
    const SETTLE_MS: u64 = 300;

    async fn try_connect() -> Option<async_nats::Client> {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
        async_nats::connect(&url).await.ok()
    }

    /// Create an Http client that routes all Discord API calls to a local wiremock server.
    /// application_id(1) is required so that followup/interaction endpoints don't error
    /// before making the HTTP request (serenity's try_application_id returns Err if unset).
    fn proxy_http(proxy_url: &str) -> Arc<serenity::http::Http> {
        use serenity::model::id::ApplicationId;
        Arc::new(
            serenity::http::HttpBuilder::new("fake-token")
                .proxy(proxy_url)
                .ratelimiter_disabled(true)
                .application_id(ApplicationId::new(1))
                .build(),
        )
    }

    async fn start_processor(
        client: async_nats::Client,
        prefix: &str,
        http: Arc<serenity::http::Http>,
    ) {
        use crate::bridge::PairingState;
        use crate::config::ReplyToMode;
        use discord_nats::MessagePublisher;
        let pairing_state = Arc::new(PairingState::new());
        let publisher = MessagePublisher::new(client.clone(), prefix);
        let processor = OutboundProcessor::new(
            http,
            client,
            publisher,
            prefix.to_string(),
            None,
            pairing_state,
            None,
            ReplyToMode::First,
            None,
        );
        tokio::spawn(async move {
            let _ = processor.run().await;
        });
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
        let server = MockServer::start().await;
        let prefix = format!("test-out-send-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/channels/100/messages"))
            .and(body_string_contains("Hello from test"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::message_send(&prefix),
            &SendMessageCommand {
                channel_id: 100,
                content: "Hello from test".to_string(),
                embeds: vec![],
                reply_to_message_id: None,
                files: vec![],
                components: vec![],
                as_voice: false,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    #[tokio::test]
    async fn test_send_message_with_reply_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-reply-{}", uuid::Uuid::new_v4().simple());

        // serenity serializes the reply as "message_reference" in the body
        Mock::given(method("POST"))
            .and(path("/api/v10/channels/100/messages"))
            .and(body_string_contains("message_reference"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::message_send(&prefix),
            &SendMessageCommand {
                channel_id: 100,
                content: "Reply message".to_string(),
                embeds: vec![],
                reply_to_message_id: Some(50),
                files: vec![],
                components: vec![],
                as_voice: false,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── edit_message ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_edit_message_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-edit-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("PATCH"))
            .and(path("/api/v10/channels/100/messages/1"))
            .and(body_string_contains("Edited"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::message_edit(&prefix),
            &EditMessageCommand {
                channel_id: 100,
                message_id: 1,
                content: Some("Edited".to_string()),
                embeds: vec![],
                components: vec![],
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── delete_message ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_message_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-del-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("DELETE"))
            .and(path("/api/v10/channels/100/messages/1"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::message_delete(&prefix),
            &DeleteMessageCommand {
                channel_id: 100,
                message_id: 1,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── interaction_respond ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_interaction_respond_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-resp-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/interactions/9999/fake-token/callback"))
            .and(body_string_contains("pong"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::interaction_respond(&prefix),
            &InteractionRespondCommand {
                interaction_id: 9999,
                interaction_token: "fake-token".to_string(),
                content: Some("pong".to_string()),
                embeds: vec![],
                ephemeral: false,
                components: vec![],
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── interaction_defer ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_interaction_defer_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-defer-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/interactions/9999/fake-token/callback"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::interaction_defer(&prefix),
            &InteractionDeferCommand {
                interaction_id: 9999,
                interaction_token: "fake-token".to_string(),
                ephemeral: true,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── interaction_followup ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_interaction_followup_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-fup-{}", uuid::Uuid::new_v4().simple());

        // serenity sends followup via webhook: POST /api/v10/webhooks/{app_id}/{token}
        Mock::given(method("POST"))
            .and(path_regex(r"^/api/v10/webhooks/[0-9]+/fake-token$"))
            .and(body_string_contains("followup text"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::interaction_followup(&prefix),
            &InteractionFollowupCommand {
                interaction_token: "fake-token".to_string(),
                content: Some("followup text".to_string()),
                embeds: vec![],
                ephemeral: false,
                session_id: None,
                files: vec![],
                components: vec![],
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── add_reaction ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_add_reaction_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-react-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("PUT"))
            .and(path_regex(
                r"^/api/v10/channels/100/messages/1/reactions/[^/]+/@me$",
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::reaction_add(&prefix),
            &AddReactionCommand {
                channel_id: 100,
                message_id: 1,
                emoji: "👍".to_string(),
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── remove_reaction ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_remove_reaction_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-unreact-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("DELETE"))
            .and(path_regex(
                r"^/api/v10/channels/100/messages/1/reactions/[^/]+/@me$",
            ))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::reaction_remove(&prefix),
            &RemoveReactionCommand {
                channel_id: 100,
                message_id: 1,
                emoji: "👍".to_string(),
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── typing ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_typing_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-typ-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/channels/100/typing"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::channel_typing(&prefix),
            &TypingCommand { channel_id: 100 },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── modal_respond ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_modal_respond_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-modal-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/interactions/1111/fake-token/callback"))
            .and(body_string_contains("my_modal"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::interaction_modal_respond(&prefix),
            &ModalRespondCommand {
                interaction_id: 1111,
                interaction_token: "fake-token".to_string(),
                custom_id: "my_modal".to_string(),
                title: "Test Modal".to_string(),
                inputs: vec![],
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── autocomplete_respond ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_autocomplete_respond_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-ac-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/interactions/2222/fake-token/callback"))
            .and(body_string_contains("Option A"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::interaction_autocomplete_respond(&prefix),
            &AutocompleteRespondCommand {
                interaction_id: 2222,
                interaction_token: "fake-token".to_string(),
                choices: vec![AutocompleteChoice {
                    name: "Option A".to_string(),
                    value: "a".to_string(),
                }],
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── ban ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_ban_user_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-ban-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("PUT"))
            .and(path("/api/v10/guilds/200/bans/42"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::guild_ban(&prefix),
            &BanUserCommand {
                guild_id: 200,
                user_id: 42,
                reason: Some("spam".to_string()),
                delete_message_seconds: 0,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── kick ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_kick_user_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-kick-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("DELETE"))
            .and(path("/api/v10/guilds/200/members/42"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::guild_kick(&prefix),
            &KickUserCommand {
                guild_id: 200,
                user_id: 42,
                reason: None,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── timeout ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_timeout_user_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-timeout-{}", uuid::Uuid::new_v4().simple());

        // serenity encodes the timeout as "communication_disabled_until" in the member PATCH
        Mock::given(method("PATCH"))
            .and(path("/api/v10/guilds/200/members/42"))
            .and(body_string_contains("communication_disabled_until"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::guild_timeout(&prefix),
            &TimeoutUserCommand {
                guild_id: 200,
                user_id: 42,
                duration_secs: 3600,
                reason: Some("cooling off".to_string()),
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── create_channel ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_channel_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-ch-create-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/guilds/200/channels"))
            .and(body_string_contains("new-channel"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::channel_create(&prefix),
            &CreateChannelCommand {
                guild_id: 200,
                name: "new-channel".to_string(),
                channel_type: ChannelType::GuildText,
                category_id: None,
                topic: None,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── edit_channel ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_edit_channel_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-ch-edit-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("PATCH"))
            .and(path("/api/v10/channels/300"))
            .and(body_string_contains("renamed"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::channel_edit(&prefix),
            &EditChannelCommand {
                channel_id: 300,
                name: Some("renamed".to_string()),
                topic: None,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── delete_channel ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_channel_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-ch-del-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("DELETE"))
            .and(path("/api/v10/channels/300"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::channel_delete(&prefix),
            &DeleteChannelCommand { channel_id: 300 },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── create_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_role_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-role-create-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/guilds/200/roles"))
            .and(body_string_contains("VIP"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::role_create(&prefix),
            &CreateRoleCommand {
                guild_id: 200,
                name: "VIP".to_string(),
                color: Some(0xFFD700),
                hoist: true,
                mentionable: false,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── assign_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_assign_role_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-role-assign-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("PUT"))
            .and(path("/api/v10/guilds/200/members/42/roles/111"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::role_assign(&prefix),
            &AssignRoleCommand {
                guild_id: 200,
                user_id: 42,
                role_id: 111,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── remove_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_remove_role_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-role-remove-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("DELETE"))
            .and(path("/api/v10/guilds/200/members/42/roles/111"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::role_remove(&prefix),
            &RemoveRoleCommand {
                guild_id: 200,
                user_id: 42,
                role_id: 111,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── delete_role ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_role_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-role-del-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("DELETE"))
            .and(path("/api/v10/guilds/200/roles/111"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::role_delete(&prefix),
            &DeleteRoleCommand {
                guild_id: 200,
                role_id: 111,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── pin ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_pin_message_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-pin-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("PUT"))
            .and(path("/api/v10/channels/100/pins/50"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::message_pin(&prefix),
            &PinMessageCommand {
                channel_id: 100,
                message_id: 50,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── unpin ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_unpin_message_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-unpin-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("DELETE"))
            .and(path("/api/v10/channels/100/pins/50"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::message_unpin(&prefix),
            &UnpinMessageCommand {
                channel_id: 100,
                message_id: 50,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── bulk_delete ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_bulk_delete_messages_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-bulk-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/channels/100/messages/bulk-delete"))
            .and(body_string_contains("messages"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::message_bulk_delete(&prefix),
            &BulkDeleteMessagesCommand {
                channel_id: 100,
                message_ids: vec![1, 2, 3],
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── create_thread ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_thread_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-thread-create-{}", uuid::Uuid::new_v4().simple());

        Mock::given(method("POST"))
            .and(path("/api/v10/channels/100/threads"))
            .and(body_string_contains("Discussion"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::thread_create(&prefix),
            &CreateThreadCommand {
                channel_id: 100,
                name: "Discussion".to_string(),
                message_id: None,
                auto_archive_mins: 1440,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── archive_thread ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_archive_thread_command_is_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-thread-archive-{}", uuid::Uuid::new_v4().simple());

        // serenity's edit_thread calls PATCH /channels/{id}
        Mock::given(method("PATCH"))
            .and(path("/api/v10/channels/100"))
            .and(body_string_contains("archived"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;
        publish(
            &client,
            &subjects::agent::thread_archive(&prefix),
            &ArchiveThreadCommand {
                channel_id: 100,
                locked: false,
            },
        )
        .await;

        tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
        server.verify().await;
    }

    // ── multiple commands in sequence ─────────────────────────────────────────

    #[tokio::test]
    async fn test_multiple_commands_all_consumed() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let server = MockServer::start().await;
        let prefix = format!("test-out-multi-{}", uuid::Uuid::new_v4().simple());

        // Expect exactly 3 POSTs to channels 100, 101, 102
        Mock::given(method("POST"))
            .and(path_regex(r"^/api/v10/channels/10[0-9]+/messages$"))
            .respond_with(ResponseTemplate::new(204))
            .expect(3)
            .mount(&server)
            .await;

        start_processor(client.clone(), &prefix, proxy_http(&server.uri())).await;

        for i in 0..3u64 {
            publish(
                &client,
                &subjects::agent::message_send(&prefix),
                &SendMessageCommand {
                    channel_id: 100 + i,
                    content: format!("message {}", i),
                    embeds: vec![],
                    reply_to_message_id: None,
                    files: vec![],
                    components: vec![],
                    as_voice: false,
                },
            )
            .await;
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        server.verify().await;
    }
}
