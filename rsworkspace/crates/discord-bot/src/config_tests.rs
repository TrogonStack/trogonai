#[cfg(test)]
mod tests {
    use crate::config::{Config, ReadEnv};
    use discord_types::{AccessConfig, DmPolicy, GuildPolicy};
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::NamedTempFile;

    struct InMemoryEnv(HashMap<&'static str, &'static str>);

    impl InMemoryEnv {
        fn new(pairs: &[(&'static str, &'static str)]) -> Self {
            Self(pairs.iter().cloned().collect())
        }
    }

    impl ReadEnv for InMemoryEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|v| v.to_string())
        }
    }

    fn write_toml(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    // ── from_file ─────────────────────────────────────────────────────────────

    #[test]
    fn test_from_file_minimal() {
        let toml = r#"
[discord]
bot_token = "BOT-TOKEN-123"

[nats]
servers = ["nats://localhost:4222"]
prefix = "test"
"#;
        let f = write_toml(toml);
        let cfg = Config::from_file(f.path().to_str().unwrap()).unwrap();
        assert_eq!(cfg.discord.bot_token, "BOT-TOKEN-123");
        assert_eq!(cfg.nats.prefix, "test");
        assert_eq!(cfg.nats.servers, vec!["nats://localhost:4222"]);
    }

    #[test]
    fn test_from_file_with_access_config() {
        let toml = r#"
[discord]
bot_token = "SECRET"

[discord.access]
dm_policy = "open"
guild_policy = "allowlist"
admin_users = [111, 222]
user_allowlist = [333, 444]
guild_allowlist = [555]

[nats]
servers = ["nats://localhost:4222"]
prefix = "prod"
"#;
        let f = write_toml(toml);
        let cfg = Config::from_file(f.path().to_str().unwrap()).unwrap();
        assert_eq!(cfg.discord.access.dm_policy, DmPolicy::Open);
        assert_eq!(cfg.discord.access.guild_policy, GuildPolicy::Allowlist);
        assert_eq!(cfg.discord.access.admin_users, vec![111, 222]);
        assert_eq!(cfg.discord.access.user_allowlist, vec![333, 444]);
        assert_eq!(cfg.discord.access.guild_allowlist, vec![555]);
    }

    #[test]
    fn test_from_file_default_access() {
        let toml = r#"
[discord]
bot_token = "TOK"

[nats]
servers = ["nats://localhost:4222"]
prefix = "dev"
"#;
        let f = write_toml(toml);
        let cfg = Config::from_file(f.path().to_str().unwrap()).unwrap();
        let default_access = AccessConfig::default();
        assert_eq!(cfg.discord.access.dm_policy, default_access.dm_policy);
        assert_eq!(cfg.discord.access.guild_policy, default_access.guild_policy);
        assert!(cfg.discord.access.admin_users.is_empty());
        assert!(cfg.discord.access.user_allowlist.is_empty());
        assert!(cfg.discord.access.guild_allowlist.is_empty());
    }

    #[test]
    fn test_from_file_missing_returns_error() {
        let result = Config::from_file("/nonexistent/path/config.toml");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Failed to read config file"));
    }

    #[test]
    fn test_from_file_invalid_toml_returns_error() {
        let f = write_toml("this is not valid toml !!!");
        let result = Config::from_file(f.path().to_str().unwrap());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Failed to parse config file"));
    }

    #[test]
    fn test_from_file_multiple_nats_servers() {
        let toml = r#"
[discord]
bot_token = "TOK"

[nats]
servers = ["nats://host1:4222", "nats://host2:4222", "nats://host3:4222"]
prefix = "prod"
"#;
        let f = write_toml(toml);
        let cfg = Config::from_file(f.path().to_str().unwrap()).unwrap();
        assert_eq!(cfg.nats.servers.len(), 3);
        assert_eq!(cfg.nats.servers[1], "nats://host2:4222");
    }

    // ── from_env ──────────────────────────────────────────────────────────────

    #[test]
    fn test_from_env_missing_token_returns_error() {
        let env = InMemoryEnv::new(&[]);
        let result = Config::from_env_impl(&env);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_env_reads_token() {
        let env = InMemoryEnv::new(&[
            ("DISCORD_BOT_TOKEN", "env-token-abc"),
            ("NATS_URL", "nats://nats-env:4222"),
            ("DISCORD_PREFIX", "staging"),
        ]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.discord.bot_token, "env-token-abc");
        assert_eq!(cfg.nats.prefix, "staging");
    }

    #[test]
    fn test_from_env_defaults_nats_url() {
        let env = InMemoryEnv::new(&[("DISCORD_BOT_TOKEN", "tok")]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.nats.prefix, "prod");
        assert!(!cfg.nats.servers.is_empty());
    }

    #[test]
    fn test_from_env_guild_policy_open() {
        let env = InMemoryEnv::new(&[
            ("DISCORD_BOT_TOKEN", "tok"),
            ("DISCORD_GUILD_POLICY", "open"),
        ]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.discord.access.guild_policy, GuildPolicy::Open);
    }

    #[test]
    fn test_from_env_guild_allowlist_parsed() {
        let env = InMemoryEnv::new(&[
            ("DISCORD_BOT_TOKEN", "tok"),
            ("DISCORD_GUILD_POLICY", "allowlist"),
            ("DISCORD_GUILD_ALLOWLIST", "111, 222, 333"),
        ]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.discord.access.guild_policy, GuildPolicy::Allowlist);
        assert_eq!(cfg.discord.access.guild_allowlist, vec![111, 222, 333]);
    }

    #[test]
    fn test_from_env_dm_policy_disabled() {
        let env = InMemoryEnv::new(&[
            ("DISCORD_BOT_TOKEN", "tok"),
            ("DISCORD_DM_POLICY", "disabled"),
        ]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.discord.access.dm_policy, DmPolicy::Disabled);
    }

    #[test]
    fn test_from_env_user_and_admin_lists() {
        let env = InMemoryEnv::new(&[
            ("DISCORD_BOT_TOKEN", "tok"),
            ("DISCORD_USER_ALLOWLIST", "10,20,30"),
            ("DISCORD_ADMIN_USERS", "999"),
        ]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.discord.access.user_allowlist, vec![10, 20, 30]);
        assert_eq!(cfg.discord.access.admin_users, vec![999]);
    }

    #[test]
    fn test_from_env_require_mention_true() {
        let env = InMemoryEnv::new(&[
            ("DISCORD_BOT_TOKEN", "tok"),
            ("DISCORD_REQUIRE_MENTION", "true"),
        ]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert!(cfg.discord.access.require_mention);
    }

    #[test]
    fn test_from_env_require_mention_default_false() {
        let env = InMemoryEnv::new(&[("DISCORD_BOT_TOKEN", "tok")]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert!(!cfg.discord.access.require_mention);
    }

    #[test]
    fn test_from_env_defaults_all_policies() {
        let env = InMemoryEnv::new(&[("DISCORD_BOT_TOKEN", "tok")]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.discord.access.guild_policy, GuildPolicy::Allowlist);
        assert_eq!(cfg.discord.access.dm_policy, DmPolicy::Allowlist);
        assert!(cfg.discord.access.guild_allowlist.is_empty());
        assert!(cfg.discord.access.user_allowlist.is_empty());
        assert!(cfg.discord.access.admin_users.is_empty());
    }

    #[test]
    fn test_from_env_channel_allowlist_parsed() {
        let env = InMemoryEnv::new(&[
            ("DISCORD_BOT_TOKEN", "tok"),
            ("DISCORD_CHANNEL_ALLOWLIST", "701, 702, 703"),
        ]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert_eq!(cfg.discord.access.channel_allowlist, vec![701, 702, 703]);
    }

    #[test]
    fn test_from_env_channel_allowlist_default_empty() {
        let env = InMemoryEnv::new(&[("DISCORD_BOT_TOKEN", "tok")]);
        let cfg = Config::from_env_impl(&env).unwrap();
        assert!(cfg.discord.access.channel_allowlist.is_empty());
    }
}
