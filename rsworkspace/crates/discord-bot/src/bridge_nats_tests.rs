//! Unit tests for DiscordBridge publish methods — no real NATS connection required.
//!
//! Uses `MockPublisher` to capture published messages and verify their contents.

#[cfg(test)]
mod tests {
    use crate::bridge::DiscordBridge;
    use discord_nats::{subjects, MockPublisher};
    use discord_types::{
        events::{MessageCreatedEvent, MessageDeletedEvent, MessageUpdatedEvent},
        AccessConfig, DmPolicy, GuildPolicy,
    };
    use serenity::model::channel::Message as SerenityMessage;
    use serenity::model::event::MessageUpdateEvent;
    use serenity::model::id::{ChannelId, GuildId, MessageId};

    fn make_bridge(prefix: &str) -> (DiscordBridge<MockPublisher>, MockPublisher) {
        let mock = MockPublisher::new(prefix);
        let bridge = DiscordBridge::with_publisher(
            mock.clone(),
            AccessConfig {
                dm_policy: DmPolicy::Open,
                guild_policy: GuildPolicy::Allowlist,
                guild_allowlist: vec![200],
                ..Default::default()
            },
            false,
            None,
            crate::config::ReactionMode::Own,
            vec![],
            None,
            false,
            false,
            None,
            false,
            vec![],
        );
        (bridge, mock)
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

    // ── message_created ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_publish_dm_message_created() {
        let prefix = "test-dc-dm";
        let (bridge, mock) = make_bridge(prefix);

        let msg = dm_message(1, 100, 42, "Hello NATS");
        bridge.publish_message_created(&msg, None, None).await.unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, subjects::bot::message_created(prefix));

        let event: MessageCreatedEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.message.id, 1);
        assert_eq!(event.message.channel_id, 100);
        assert_eq!(event.message.guild_id, None);
        assert_eq!(event.message.author.id, 42);
        assert_eq!(event.message.content, "Hello NATS");
        assert!(event.metadata.session_id.starts_with("dc-dm-"));
    }

    #[tokio::test]
    async fn test_publish_guild_message_created() {
        let prefix = "test-dc-guild";
        let (bridge, mock) = make_bridge(prefix);

        let msg = guild_message(5, 100, 200, 42, "Guild hello");
        bridge.publish_message_created(&msg, None, None).await.unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        let event: MessageCreatedEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.message.id, 5);
        assert_eq!(event.message.guild_id, Some(200));
        assert_eq!(event.message.content, "Guild hello");
        assert!(event.metadata.session_id.contains("guild"));
    }

    #[tokio::test]
    async fn test_sequence_increments_per_event() {
        let (bridge, mock) = make_bridge("test-dc-seq");

        for i in 1u64..=3 {
            let msg = dm_message(i, 100, 42, "seq");
            bridge.publish_message_created(&msg, None, None).await.unwrap();
        }

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 3);
        let e1: MessageCreatedEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        let e2: MessageCreatedEvent = serde_json::from_value(msgs[1].1.clone()).unwrap();
        let e3: MessageCreatedEvent = serde_json::from_value(msgs[2].1.clone()).unwrap();
        assert!(e1.metadata.sequence < e2.metadata.sequence);
        assert!(e2.metadata.sequence < e3.metadata.sequence);
    }

    // ── message_deleted ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_publish_message_deleted_dm() {
        let prefix = "test-dc-del";
        let (bridge, mock) = make_bridge(prefix);

        bridge
            .publish_message_deleted(ChannelId::new(100), None, MessageId::new(99))
            .await
            .unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, subjects::bot::message_deleted(prefix));
        let event: MessageDeletedEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.message_id, 99);
        assert_eq!(event.channel_id, 100);
        assert_eq!(event.guild_id, None);
    }

    #[tokio::test]
    async fn test_publish_message_deleted_guild() {
        let prefix = "test-dc-delg";
        let (bridge, mock) = make_bridge(prefix);

        bridge
            .publish_message_deleted(
                ChannelId::new(100),
                Some(GuildId::new(200)),
                MessageId::new(77),
            )
            .await
            .unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        let event: MessageDeletedEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.message_id, 77);
        assert_eq!(event.guild_id, Some(200));
    }

    // ── message_updated ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_publish_message_updated() {
        let prefix = "test-dc-upd";
        let (bridge, mock) = make_bridge(prefix);

        let update: MessageUpdateEvent = serde_json::from_value(serde_json::json!({
            "id": "10",
            "channel_id": "100",
            "content": "edited content"
        }))
        .expect("construct MessageUpdateEvent");

        bridge.publish_message_updated(&update).await.unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, subjects::bot::message_updated(prefix));
        let event: MessageUpdatedEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.message_id, 10);
        assert_eq!(event.channel_id, 100);
        assert_eq!(event.new_content, Some("edited content".to_string()));
    }

    // ── access control ────────────────────────────────────────────────────────

    #[test]
    fn test_guild_access_check_allows_listed_guild() {
        let (bridge, _) = make_bridge("test-dc-ac");
        assert!(bridge.check_guild_access(200));
        assert!(!bridge.check_guild_access(999));
    }

    #[test]
    fn test_dm_access_check_open_policy() {
        let (bridge, _) = make_bridge("test-dc-dm-ac");
        assert!(bridge.check_dm_access(1));
        assert!(bridge.check_dm_access(99999));
    }
}
