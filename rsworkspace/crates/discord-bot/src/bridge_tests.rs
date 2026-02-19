#[cfg(test)]
mod tests {
    use crate::bridge::DiscordBridge;
    use discord_nats::MessagePublisher;
    use discord_types::{AccessConfig, DmPolicy, GuildPolicy};
    type Bridge = DiscordBridge<MessagePublisher>;
    use serenity::model::channel::Message as SerenityMessage;
    use serenity::model::user::User as SerenityUser;

    // ── JSON helpers ──────────────────────────────────────────────────────────

    fn user_json(id: u64, username: &str, bot: bool) -> serde_json::Value {
        serde_json::json!({
            "id": id.to_string(),
            "username": username,
            "global_name": null,
            "avatar": null,
            "bot": bot
        })
    }

    fn dm_message_json(
        message_id: u64,
        channel_id: u64,
        user_id: u64,
        content: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": message_id.to_string(),
            "channel_id": channel_id.to_string(),
            "author": user_json(user_id, "alice", false),
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
        })
    }

    fn guild_message_json(
        message_id: u64,
        channel_id: u64,
        guild_id: u64,
        user_id: u64,
        content: &str,
    ) -> serde_json::Value {
        let mut v = dm_message_json(message_id, channel_id, user_id, content);
        v["guild_id"] = serde_json::Value::String(guild_id.to_string());
        v
    }

    fn parse_user(json: serde_json::Value) -> SerenityUser {
        serde_json::from_value(json).expect("construct SerenityUser")
    }

    fn parse_message(json: serde_json::Value) -> SerenityMessage {
        serde_json::from_value(json).expect("construct SerenityMessage")
    }

    // ── AccessConfig (DiscordBridge delegates to this) ─────────────────────────

    fn open_access() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Open,
            guild_policy: GuildPolicy::Allowlist,
            guild_allowlist: vec![100, 200],
            user_allowlist: vec![],
            admin_users: vec![999],
            channel_allowlist: vec![],
            require_mention: false,
        }
    }

    fn allowlist_access() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Allowlist,
            guild_policy: GuildPolicy::Allowlist,
            user_allowlist: vec![10, 20, 30],
            guild_allowlist: vec![100, 200],
            admin_users: vec![999],
            channel_allowlist: vec![],
            require_mention: false,
        }
    }

    fn disabled_access() -> AccessConfig {
        AccessConfig {
            dm_policy: DmPolicy::Disabled,
            guild_policy: GuildPolicy::Disabled,
            user_allowlist: vec![10],
            guild_allowlist: vec![100],
            admin_users: vec![],
            channel_allowlist: vec![],
            require_mention: false,
        }
    }

    // ── Guild access ──────────────────────────────────────────────────────────

    #[test]
    fn test_guild_allowlist_permits_listed() {
        let cfg = allowlist_access();
        assert!(cfg.can_access_guild(100));
        assert!(cfg.can_access_guild(200));
    }

    #[test]
    fn test_guild_allowlist_blocks_unlisted() {
        let cfg = allowlist_access();
        assert!(!cfg.can_access_guild(300));
        assert!(!cfg.can_access_guild(1));
    }

    #[test]
    fn test_guild_disabled_blocks_all() {
        let cfg = disabled_access();
        assert!(!cfg.can_access_guild(100));
        assert!(!cfg.can_access_guild(1));
    }

    // ── DM access ─────────────────────────────────────────────────────────────

    #[test]
    fn test_dm_open_allows_anyone() {
        let cfg = open_access();
        assert!(cfg.can_access_dm(1));
        assert!(cfg.can_access_dm(99999));
    }

    #[test]
    fn test_dm_allowlist_permits_listed_user() {
        let cfg = allowlist_access();
        assert!(cfg.can_access_dm(10));
        assert!(cfg.can_access_dm(20));
    }

    #[test]
    fn test_dm_allowlist_blocks_unlisted_user() {
        let cfg = allowlist_access();
        assert!(!cfg.can_access_dm(11));
        assert!(!cfg.can_access_dm(99));
    }

    #[test]
    fn test_dm_allowlist_admin_bypass() {
        let cfg = allowlist_access();
        // Admin 999 is not in user_allowlist but can still DM
        assert!(cfg.can_access_dm(999));
    }

    #[test]
    fn test_dm_disabled_blocks_all() {
        let cfg = disabled_access();
        assert!(!cfg.can_access_dm(10));
        assert!(!cfg.can_access_dm(1));
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_is_admin_true() {
        assert!(allowlist_access().is_admin(999));
    }

    #[test]
    fn test_is_admin_false() {
        assert!(!allowlist_access().is_admin(10));
        assert!(!open_access().is_admin(1));
    }

    #[test]
    fn test_no_admins_configured() {
        let cfg = disabled_access();
        assert!(!cfg.is_admin(999));
    }

    // ── convert_user ──────────────────────────────────────────────────────────

    #[test]
    fn test_convert_user_basic() {
        let user = parse_user(user_json(42, "alice", false));
        let converted = Bridge::convert_user(&user);
        assert_eq!(converted.id, 42);
        assert_eq!(converted.username, "alice");
        assert!(!converted.bot);
        assert!(converted.global_name.is_none());
    }

    #[test]
    fn test_convert_user_bot_flag() {
        let user = parse_user(user_json(1, "mybot", true));
        let converted = Bridge::convert_user(&user);
        assert!(converted.bot);
    }

    #[test]
    fn test_convert_user_with_global_name() {
        let json = serde_json::json!({
            "id": "123",
            "username": "alice",
            "global_name": "Alice Wonderland",
            "avatar": null,
            "bot": false
        });
        let user = parse_user(json);
        let converted = Bridge::convert_user(&user);
        assert_eq!(converted.global_name, Some("Alice Wonderland".to_string()));
    }

    #[test]
    fn test_convert_user_large_id() {
        let user = parse_user(user_json(987654321098765432, "bigid", false));
        let converted = Bridge::convert_user(&user);
        assert_eq!(converted.id, 987654321098765432);
    }

    // ── convert_message ───────────────────────────────────────────────────────

    #[test]
    fn test_convert_dm_message_fields() {
        let json = dm_message_json(1, 100, 42, "Hello world");
        let msg = parse_message(json);
        let converted = Bridge::convert_message(&msg);

        assert_eq!(converted.id, 1);
        assert_eq!(converted.channel_id, 100);
        assert_eq!(converted.guild_id, None);
        assert_eq!(converted.author.id, 42);
        assert_eq!(converted.content, "Hello world");
        assert!(converted.attachments.is_empty());
        assert!(converted.embeds.is_empty());
        assert!(converted.edited_timestamp.is_none());
        assert!(converted.referenced_message_id.is_none());
    }

    #[test]
    fn test_convert_guild_message_has_guild_id() {
        let json = guild_message_json(5, 100, 200, 42, "Guild message");
        let msg = parse_message(json);
        let converted = Bridge::convert_message(&msg);
        assert_eq!(converted.guild_id, Some(200));
        assert_eq!(converted.channel_id, 100);
    }

    #[test]
    fn test_convert_message_empty_content() {
        let json = dm_message_json(1, 100, 42, "");
        let msg = parse_message(json);
        let converted = Bridge::convert_message(&msg);
        assert_eq!(converted.content, "");
    }

    #[test]
    fn test_convert_message_author_is_converted() {
        let json = dm_message_json(1, 100, 77, "Hi");
        let msg = parse_message(json);
        let converted = Bridge::convert_message(&msg);
        assert_eq!(converted.author.id, 77);
        assert_eq!(converted.author.username, "alice");
    }

    #[test]
    fn test_convert_message_with_attachment() {
        let json = serde_json::json!({
            "id": "1",
            "channel_id": "100",
            "author": user_json(42, "alice", false),
            "content": "file",
            "timestamp": "2024-01-01T00:00:00+00:00",
            "edited_timestamp": null,
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [{
                "id": "999",
                "filename": "image.png",
                "url": "https://cdn.discord.com/image.png",
                "proxy_url": "https://media.discord.com/image.png",
                "size": 12345,
                "content_type": "image/png"
            }],
            "embeds": [],
            "pinned": false,
            "type": 0
        });
        let msg = parse_message(json);
        let converted = Bridge::convert_message(&msg);

        assert_eq!(converted.attachments.len(), 1);
        assert_eq!(converted.attachments[0].filename, "image.png");
        assert_eq!(converted.attachments[0].size, 12345);
        assert_eq!(
            converted.attachments[0].content_type,
            Some("image/png".to_string())
        );
    }

    #[test]
    fn test_convert_message_timestamp_is_nonempty() {
        let json = dm_message_json(1, 100, 42, "ts test");
        let msg = parse_message(json);
        let converted = Bridge::convert_message(&msg);
        assert!(!converted.timestamp.is_empty());
    }
}
