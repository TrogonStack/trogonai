//! NATS integration tests for DiscordBridge — require a live NATS server.
//!
//! Run with: NATS_URL=nats://localhost:14222 cargo test bridge_nats
//!
//! Tests are skipped automatically when NATS is unreachable.

#[cfg(test)]
mod nats_tests {
    use crate::bridge::DiscordBridge;
    use discord_nats::subjects;
    use discord_types::{
        events::{MessageCreatedEvent, MessageDeletedEvent, MessageUpdatedEvent},
        AccessConfig, DmPolicy, GuildPolicy,
    };
    use futures::StreamExt;
    use serenity::model::channel::Message as SerenityMessage;
    use serenity::model::event::MessageUpdateEvent;
    use serenity::model::id::{ChannelId, GuildId, MessageId};

    const DEFAULT_NATS_URL: &str = "nats://localhost:14222";

    async fn try_connect() -> Option<async_nats::Client> {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
        async_nats::connect(&url).await.ok()
    }

    fn make_bridge(client: async_nats::Client, prefix: &str) -> DiscordBridge {
        DiscordBridge::new(
            client,
            prefix.to_string(),
            AccessConfig {
                dm_policy: DmPolicy::Open,
                guild_policy: GuildPolicy::Allowlist,
                guild_allowlist: vec![200],
                ..Default::default()
            },
            false,
            None,
        )
    }

    fn user_json(id: u64) -> serde_json::Value {
        serde_json::json!({
            "id": id.to_string(),
            "username": "testuser",
            "global_name": null,
            "avatar": null,
            "bot": false
        })
    }

    fn dm_message(
        message_id: u64,
        channel_id: u64,
        user_id: u64,
        content: &str,
    ) -> SerenityMessage {
        serde_json::from_value(serde_json::json!({
            "id": message_id.to_string(),
            "channel_id": channel_id.to_string(),
            "author": user_json(user_id),
            "content": content,
            "timestamp": "2024-01-01T00:00:00+00:00",
            "edited_timestamp": null,
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0
        }))
        .expect("construct DM SerenityMessage")
    }

    fn guild_message(
        message_id: u64,
        channel_id: u64,
        guild_id: u64,
        user_id: u64,
        content: &str,
    ) -> SerenityMessage {
        serde_json::from_value(serde_json::json!({
            "id": message_id.to_string(),
            "channel_id": channel_id.to_string(),
            "guild_id": guild_id.to_string(),
            "author": user_json(user_id),
            "content": content,
            "timestamp": "2024-01-01T00:00:00+00:00",
            "edited_timestamp": null,
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0
        }))
        .expect("construct guild SerenityMessage")
    }

    async fn recv<T: for<'de> serde::Deserialize<'de>>(sub: &mut async_nats::Subscriber) -> T {
        let raw = tokio::time::timeout(std::time::Duration::from_secs(3), sub.next())
            .await
            .expect("timed out waiting for NATS message")
            .expect("subscriber closed");
        serde_json::from_slice(&raw.payload).expect("deserialize event")
    }

    // ── message_created ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_publish_dm_message_created() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-dm-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        let subject = subjects::bot::message_created(&prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        let msg = dm_message(1, 100, 42, "Hello NATS");
        bridge.publish_message_created(&msg).await.unwrap();

        let event: MessageCreatedEvent = recv(&mut sub).await;
        assert_eq!(event.message.id, 1);
        assert_eq!(event.message.channel_id, 100);
        assert_eq!(event.message.guild_id, None);
        assert_eq!(event.message.author.id, 42);
        assert_eq!(event.message.content, "Hello NATS");
        assert!(event.metadata.session_id.starts_with("dc-dm-"));
    }

    #[tokio::test]
    async fn test_publish_guild_message_created() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-guild-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        let subject = subjects::bot::message_created(&prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        let msg = guild_message(5, 100, 200, 42, "Guild hello");
        bridge.publish_message_created(&msg).await.unwrap();

        let event: MessageCreatedEvent = recv(&mut sub).await;
        assert_eq!(event.message.id, 5);
        assert_eq!(event.message.guild_id, Some(200));
        assert_eq!(event.message.content, "Guild hello");
        assert!(event.metadata.session_id.contains("guild"));
    }

    #[tokio::test]
    async fn test_sequence_increments_per_event() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-seq-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        let subject = subjects::bot::message_created(&prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        for i in 1u64..=3 {
            let msg = dm_message(i, 100, 42, "seq");
            bridge.publish_message_created(&msg).await.unwrap();
        }

        let e1: MessageCreatedEvent = recv(&mut sub).await;
        let e2: MessageCreatedEvent = recv(&mut sub).await;
        let e3: MessageCreatedEvent = recv(&mut sub).await;
        assert!(e1.metadata.sequence < e2.metadata.sequence);
        assert!(e2.metadata.sequence < e3.metadata.sequence);
    }

    // ── message_deleted ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_publish_message_deleted_dm() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-del-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        let subject = subjects::bot::message_deleted(&prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        bridge
            .publish_message_deleted(ChannelId::new(100), None, MessageId::new(99))
            .await
            .unwrap();

        let event: MessageDeletedEvent = recv(&mut sub).await;
        assert_eq!(event.message_id, 99);
        assert_eq!(event.channel_id, 100);
        assert_eq!(event.guild_id, None);
    }

    #[tokio::test]
    async fn test_publish_message_deleted_guild() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-delg-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        let subject = subjects::bot::message_deleted(&prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        bridge
            .publish_message_deleted(
                ChannelId::new(100),
                Some(GuildId::new(200)),
                MessageId::new(77),
            )
            .await
            .unwrap();

        let event: MessageDeletedEvent = recv(&mut sub).await;
        assert_eq!(event.message_id, 77);
        assert_eq!(event.guild_id, Some(200));
    }

    // ── message_updated ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_publish_message_updated() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-upd-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        let subject = subjects::bot::message_updated(&prefix);
        let mut sub = client.subscribe(subject).await.unwrap();

        let update: MessageUpdateEvent = serde_json::from_value(serde_json::json!({
            "id": "10",
            "channel_id": "100",
            "content": "edited content"
        }))
        .expect("construct MessageUpdateEvent");

        bridge.publish_message_updated(&update).await.unwrap();

        let event: MessageUpdatedEvent = recv(&mut sub).await;
        assert_eq!(event.message_id, 10);
        assert_eq!(event.channel_id, 100);
        assert_eq!(event.new_content, Some("edited content".to_string()));
    }

    // ── access control ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_guild_access_check_allows_listed_guild() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-ac-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        // guild 200 is in the allowlist
        assert!(bridge.check_guild_access(200));
        assert!(!bridge.check_guild_access(999));
    }

    #[tokio::test]
    async fn test_dm_access_check_open_policy() {
        let Some(client) = try_connect().await else {
            eprintln!("SKIP: NATS not available");
            return;
        };
        let prefix = format!("test-dc-dm-ac-{}", uuid::Uuid::new_v4().simple());
        let bridge = make_bridge(client.clone(), &prefix);

        // dm_policy is Open
        assert!(bridge.check_dm_access(1));
        assert!(bridge.check_dm_access(99999));
    }
}
