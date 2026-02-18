//! Configuration management for discord-bot

#[path = "config_tests.rs"]
mod config_tests;

use anyhow::{Context, Result};
use discord_nats::NatsConfig;
use discord_types::{AccessConfig, DmPolicy, GuildPolicy};
use serde::{Deserialize, Serialize};
use std::fs;

/// Abstraction over environment variable reading.
/// `SystemEnv` delegates to `std::env::var`; `InMemoryEnv` is used in tests.
pub trait ReadEnv {
    fn var(&self, key: &str) -> Option<String>;

    /// Return the value of `key`, or `default` if unset.
    fn var_or(&self, key: &str, default: &str) -> String {
        self.var(key).unwrap_or_else(|| default.to_string())
    }
}

/// Live implementation: delegates to `std::env::var`.
pub struct SystemEnv;

impl ReadEnv for SystemEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Complete bot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub discord: DiscordBotConfig,
    pub nats: NatsConfig,
}

/// Discord bot specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordBotConfig {
    /// Bot token from the Discord developer portal
    #[serde(default = "default_bot_token")]
    pub bot_token: String,
    /// Publish presence update events to NATS (requires GUILD_PRESENCES privileged intent)
    #[serde(default)]
    pub presence_enabled: bool,
    /// When set, slash commands are registered to this guild (instant propagation).
    /// When absent, commands are registered globally (takes ~1 hour to propagate).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guild_commands_guild_id: Option<u64>,
    /// Access control configuration
    #[serde(default)]
    pub access: AccessConfig,
}

impl Config {
    /// Load configuration from a TOML file
    pub fn from_file(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path))?;

        Ok(config)
    }

    /// Load from real environment variables.
    pub fn from_env() -> Result<Self> {
        Self::from_env_impl(&SystemEnv)
    }

    /// Load from any `ReadEnv` implementation (useful for testing with `InMemoryEnv`).
    pub fn from_env_impl<E: ReadEnv>(env: &E) -> Result<Self> {
        let bot_token = env.var("DISCORD_BOT_TOKEN").context("DISCORD_BOT_TOKEN not set")?;

        let nats_url = env.var_or("NATS_URL", "localhost:4222");

        let prefix = env.var_or("DISCORD_PREFIX", "prod");

        let guild_policy = match env
            .var_or("DISCORD_GUILD_POLICY", "allowlist")
            .to_lowercase()
            .as_str()
        {
            "open" => GuildPolicy::Open,
            "disabled" => GuildPolicy::Disabled,
            _ => GuildPolicy::Allowlist,
        };

        let guild_allowlist = parse_id_list(&env.var("DISCORD_GUILD_ALLOWLIST").unwrap_or_default());

        let dm_policy = match env
            .var_or("DISCORD_DM_POLICY", "allowlist")
            .to_lowercase()
            .as_str()
        {
            "open" => DmPolicy::Open,
            "disabled" => DmPolicy::Disabled,
            _ => DmPolicy::Allowlist,
        };

        let user_allowlist = parse_id_list(&env.var("DISCORD_USER_ALLOWLIST").unwrap_or_default());

        let admin_users = parse_id_list(&env.var("DISCORD_ADMIN_USERS").unwrap_or_default());

        let channel_allowlist =
            parse_id_list(&env.var("DISCORD_CHANNEL_ALLOWLIST").unwrap_or_default());

        let require_mention = env
            .var_or("DISCORD_REQUIRE_MENTION", "false")
            .to_lowercase()
            == "true";

        let presence_enabled = env
            .var_or("DISCORD_BRIDGE_PRESENCE", "false")
            .to_lowercase()
            == "true";

        let guild_commands_guild_id = env
            .var("DISCORD_GUILD_COMMANDS_GUILD_ID")
            .and_then(|s| s.parse::<u64>().ok());

        Ok(Config {
            discord: DiscordBotConfig {
                bot_token,
                presence_enabled,
                guild_commands_guild_id,
                access: AccessConfig {
                    dm_policy,
                    guild_policy,
                    admin_users,
                    user_allowlist,
                    guild_allowlist,
                    channel_allowlist,
                    require_mention,
                },
            },
            nats: NatsConfig::from_url(nats_url, prefix),
        })
    }
}

fn default_bot_token() -> String {
    std::env::var("DISCORD_BOT_TOKEN").unwrap_or_default()
}

fn parse_id_list(s: &str) -> Vec<u64> {
    s.split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .filter_map(|x| x.parse::<u64>().ok())
        .collect()
}
