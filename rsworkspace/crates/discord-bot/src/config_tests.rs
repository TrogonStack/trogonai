#[cfg(test)]
mod tests {
    use crate::config::Config;
    use discord_types::{AccessConfig, DmPolicy, GuildPolicy};
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    /// Serialize all tests that read/write process-wide env vars to avoid races.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

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
    //
    // These three tests all mutate DISCORD_BOT_TOKEN (a process-wide resource)
    // and must therefore run sequentially under ENV_MUTEX.

    #[test]
    fn test_from_env_missing_token_returns_error() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("DISCORD_BOT_TOKEN");
        let result = Config::from_env();
        assert!(result.is_err());
    }

    #[test]
    fn test_from_env_reads_token() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("DISCORD_BOT_TOKEN", "env-token-abc");
        std::env::set_var("NATS_URL", "nats://nats-env:4222");
        std::env::set_var("DISCORD_PREFIX", "staging");

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.discord.bot_token, "env-token-abc");
        assert_eq!(cfg.nats.prefix, "staging");

        std::env::remove_var("DISCORD_BOT_TOKEN");
        std::env::remove_var("NATS_URL");
        std::env::remove_var("DISCORD_PREFIX");
    }

    #[test]
    fn test_from_env_defaults_nats_url() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("DISCORD_BOT_TOKEN", "tok");
        std::env::remove_var("NATS_URL");
        std::env::remove_var("DISCORD_PREFIX");

        let cfg = Config::from_env().unwrap();
        assert_eq!(cfg.nats.prefix, "prod");
        // servers should include a localhost address
        assert!(!cfg.nats.servers.is_empty());

        std::env::remove_var("DISCORD_BOT_TOKEN");
    }
}
