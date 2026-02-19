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
        bridge
            .publish_message_created(&msg, None, None)
            .await
            .unwrap();

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
        bridge
            .publish_message_created(&msg, None, None)
            .await
            .unwrap();

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
            bridge
                .publish_message_created(&msg, None, None)
                .await
                .unwrap();
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

    // ── PairingState ──────────────────────────────────────────────────────────

    #[test]
    fn test_pairing_start_returns_code_and_expiry() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        let result = ps.start_pairing(42, 100);
        assert!(result.is_some());
        let (code, expires_at) = result.unwrap();
        assert_eq!(code.len(), 6);
        assert!(expires_at > 0);
    }

    #[test]
    fn test_pairing_start_reuses_existing_code() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        let (code1, _) = ps.start_pairing(42, 100).unwrap();
        let (code2, _) = ps.start_pairing(42, 100).unwrap();
        assert_eq!(code1, code2, "second call must reuse same code");
    }

    #[test]
    fn test_pairing_start_returns_none_when_already_approved() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        let (code, _) = ps.start_pairing(42, 100).unwrap();
        ps.approve_by_code(&code);
        assert!(
            ps.start_pairing(42, 100).is_none(),
            "approved user must return None"
        );
    }

    #[test]
    fn test_pairing_approve_by_code_succeeds() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        let (code, _) = ps.start_pairing(42, 100).unwrap();
        let result = ps.approve_by_code(&code);
        assert_eq!(result, Some((42, 100)));
        assert!(ps.is_paired(42));
    }

    #[test]
    fn test_pairing_approve_unknown_code_returns_none() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        assert!(ps.approve_by_code("XXXXXX").is_none());
    }

    #[test]
    fn test_pairing_reject_by_code_succeeds() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        let (code, _) = ps.start_pairing(42, 100).unwrap();
        let result = ps.reject_by_code(&code);
        assert_eq!(result, Some((42, 100)));
        assert!(!ps.is_paired(42), "rejected user must not be paired");
    }

    #[test]
    fn test_pairing_reject_unknown_code_returns_none() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        assert!(ps.reject_by_code("ZZZZZZ").is_none());
    }

    #[test]
    fn test_pairing_is_not_paired_initially() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        assert!(!ps.is_paired(99));
    }

    #[test]
    fn test_pairing_approve_removes_from_pending() {
        use crate::bridge::PairingState;
        let ps = PairingState::new();
        let (code, _) = ps.start_pairing(42, 100).unwrap();
        ps.approve_by_code(&code);
        // Trying to approve same code again must fail
        assert!(ps.approve_by_code(&code).is_none());
    }

    // ── should_publish_reaction ───────────────────────────────────────────────

    fn make_bridge_with_reaction(
        prefix: &str,
        mode: crate::config::ReactionMode,
        allowlist: Vec<u64>,
    ) -> DiscordBridge<MockPublisher> {
        let mock = MockPublisher::new(prefix);
        DiscordBridge::with_publisher(
            mock,
            discord_types::AccessConfig::default(),
            false,
            None,
            mode,
            allowlist,
            None,
            false,
            false,
            None,
            false,
            vec![],
        )
    }

    #[test]
    fn test_reaction_mode_off_always_false() {
        let b = make_bridge_with_reaction("t", crate::config::ReactionMode::Off, vec![]);
        assert!(!b.should_publish_reaction(true, Some(1)));
        assert!(!b.should_publish_reaction(false, Some(1)));
        assert!(!b.should_publish_reaction(true, None));
    }

    #[test]
    fn test_reaction_mode_own_only_bot_messages() {
        let b = make_bridge_with_reaction("t", crate::config::ReactionMode::Own, vec![]);
        assert!(b.should_publish_reaction(true, Some(1)));
        assert!(!b.should_publish_reaction(false, Some(1)));
        assert!(b.should_publish_reaction(true, None));
    }

    #[test]
    fn test_reaction_mode_all_always_true() {
        let b = make_bridge_with_reaction("t", crate::config::ReactionMode::All, vec![]);
        assert!(b.should_publish_reaction(false, Some(999)));
        assert!(b.should_publish_reaction(true, None));
    }

    #[test]
    fn test_reaction_mode_allowlist_in_list() {
        let b = make_bridge_with_reaction("t", crate::config::ReactionMode::Allowlist, vec![42]);
        assert!(b.should_publish_reaction(false, Some(42)));
    }

    #[test]
    fn test_reaction_mode_allowlist_not_in_list() {
        let b = make_bridge_with_reaction("t", crate::config::ReactionMode::Allowlist, vec![42]);
        assert!(!b.should_publish_reaction(false, Some(99)));
    }

    #[test]
    fn test_reaction_mode_allowlist_no_user_id() {
        let b = make_bridge_with_reaction("t", crate::config::ReactionMode::Allowlist, vec![42]);
        assert!(!b.should_publish_reaction(false, None));
    }

    // ── check_require_mention ─────────────────────────────────────────────────

    fn make_bridge_require_mention(require: bool) -> DiscordBridge<MockPublisher> {
        let mock = MockPublisher::new("t");
        DiscordBridge::with_publisher(
            mock,
            discord_types::AccessConfig {
                require_mention: require,
                ..Default::default()
            },
            false,
            None,
            crate::config::ReactionMode::Off,
            vec![],
            None,
            false,
            false,
            None,
            false,
            vec![],
        )
    }

    #[test]
    fn test_require_mention_disabled_always_passes() {
        let b = make_bridge_require_mention(false);
        assert!(b.check_require_mention(&[]));
    }

    #[test]
    fn test_require_mention_bot_id_zero_passes() {
        // bot_user_id = 0 (not set yet) → skip check
        let b = make_bridge_require_mention(true);
        assert_eq!(b.bot_user_id(), 0);
        assert!(b.check_require_mention(&[]));
    }

    #[test]
    fn test_require_mention_with_matching_mention() {
        let b = make_bridge_require_mention(true);
        b.set_bot_user_id(42);
        let user: serenity::model::user::User = serde_json::from_value(serde_json::json!({
            "id": "42", "username": "bot", "global_name": null,
            "avatar": null, "bot": true
        }))
        .unwrap();
        assert!(b.check_require_mention(&[user]));
    }

    #[test]
    fn test_require_mention_without_matching_mention() {
        let b = make_bridge_require_mention(true);
        b.set_bot_user_id(42);
        let user: serenity::model::user::User = serde_json::from_value(serde_json::json!({
            "id": "99", "username": "other", "global_name": null,
            "avatar": null, "bot": false
        }))
        .unwrap();
        assert!(!b.check_require_mention(&[user]));
    }

    // ── set_bot_user_id / bot_user_id ─────────────────────────────────────────

    #[test]
    fn test_set_and_get_bot_user_id() {
        let (bridge, _) = make_bridge("t-bot-id");
        assert_eq!(bridge.bot_user_id(), 0);
        bridge.set_bot_user_id(12345);
        assert_eq!(bridge.bot_user_id(), 12345);
    }

    // ── check_dm_access with pairing policy ───────────────────────────────────

    #[test]
    fn test_dm_access_pairing_policy_not_paired() {
        use discord_types::{AccessConfig, DmPolicy};
        let mock = MockPublisher::new("t");
        let bridge = DiscordBridge::with_publisher(
            mock,
            AccessConfig {
                dm_policy: DmPolicy::Pairing,
                ..Default::default()
            },
            false,
            None,
            crate::config::ReactionMode::Off,
            vec![],
            None,
            false,
            false,
            None,
            false,
            vec![],
        );
        assert!(!bridge.check_dm_access(42));
    }

    #[test]
    fn test_dm_access_pairing_policy_after_approve() {
        use discord_types::{AccessConfig, DmPolicy};
        let mock = MockPublisher::new("t");
        let bridge = DiscordBridge::with_publisher(
            mock,
            AccessConfig {
                dm_policy: DmPolicy::Pairing,
                ..Default::default()
            },
            false,
            None,
            crate::config::ReactionMode::Off,
            vec![],
            None,
            false,
            false,
            None,
            false,
            vec![],
        );
        let (code, _) = bridge.try_start_pairing(42, 100).unwrap();
        bridge.pairing_state.approve_by_code(&code);
        assert!(bridge.check_dm_access(42));
    }

    #[test]
    fn test_dm_access_pairing_admin_bypasses_pairing() {
        use discord_types::{AccessConfig, DmPolicy};
        let mock = MockPublisher::new("t");
        let bridge = DiscordBridge::with_publisher(
            mock,
            AccessConfig {
                dm_policy: DmPolicy::Pairing,
                admin_users: vec![42],
                ..Default::default()
            },
            false,
            None,
            crate::config::ReactionMode::Off,
            vec![],
            None,
            false,
            false,
            None,
            false,
            vec![],
        );
        assert!(bridge.check_dm_access(42), "admin must bypass pairing");
    }

    // ── check_channel_access ──────────────────────────────────────────────────

    #[test]
    fn test_channel_access_open_by_default() {
        let (bridge, _) = make_bridge("t-ch");
        // default config has no channel allowlist → all channels allowed
        assert!(bridge.check_channel_access(999));
    }

    #[test]
    fn test_channel_access_allowlist_blocks_unlisted() {
        use discord_types::AccessConfig;
        let mock = MockPublisher::new("t");
        let bridge = DiscordBridge::with_publisher(
            mock,
            AccessConfig {
                channel_allowlist: vec![50],
                ..Default::default()
            },
            false,
            None,
            crate::config::ReactionMode::Off,
            vec![],
            None,
            false,
            false,
            None,
            false,
            vec![],
        );
        assert!(bridge.check_channel_access(50));
        assert!(!bridge.check_channel_access(51));
    }

    // ── publish_guild_member_add / remove ─────────────────────────────────────

    #[tokio::test]
    async fn test_publish_guild_member_add() {
        use discord_nats::subjects;
        use discord_types::events::GuildMemberAddEvent;

        let prefix = "t-member-add";
        let (bridge, mock) = make_bridge(prefix);

        let member: serenity::model::guild::Member = serde_json::from_value(serde_json::json!({
            "guild_id": "200",
            "user": {"id": "42", "username": "alice", "global_name": null, "avatar": null, "bot": false},
            "nick": null,
            "roles": [],
            "deaf": false,
            "mute": false,
            "flags": 0
        })).unwrap();

        bridge.publish_guild_member_add(200, &member).await.unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, subjects::bot::guild_member_add(prefix));
        let event: GuildMemberAddEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.guild_id, 200);
        assert_eq!(event.member.user.id, 42);
        assert_eq!(event.member.user.username, "alice");
    }

    #[tokio::test]
    async fn test_publish_guild_member_remove() {
        use discord_nats::subjects;
        use discord_types::events::GuildMemberRemoveEvent;

        let prefix = "t-member-remove";
        let (bridge, mock) = make_bridge(prefix);

        let user: serenity::model::user::User = serde_json::from_value(serde_json::json!({
            "id": "42", "username": "alice", "global_name": null,
            "avatar": null, "bot": false
        }))
        .unwrap();

        bridge
            .publish_guild_member_remove(200, &user)
            .await
            .unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, subjects::bot::guild_member_remove(prefix));
        let event: GuildMemberRemoveEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.guild_id, 200);
        assert_eq!(event.user.id, 42);
    }

    // ── publish_pairing_requested ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_publish_pairing_requested() {
        use discord_nats::subjects;
        use discord_types::events::PairingRequestedEvent;

        let prefix = "t-pair";
        let (bridge, mock) = make_bridge(prefix);

        bridge
            .publish_pairing_requested(42, "alice", "ABC123", 9999)
            .await
            .unwrap();

        let msgs = mock.published_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, subjects::bot::pairing_requested(prefix));
        let event: PairingRequestedEvent = serde_json::from_value(msgs[0].1.clone()).unwrap();
        assert_eq!(event.user_id, 42);
        assert_eq!(event.username, "alice");
        assert_eq!(event.code, "ABC123");
        assert_eq!(event.expires_at, 9999);
    }
}
