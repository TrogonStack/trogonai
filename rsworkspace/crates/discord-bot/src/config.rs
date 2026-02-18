//! Configuration management for discord-bot

#[path = "config_tests.rs"]
mod config_tests;

use anyhow::{Context, Result};
use discord_nats::NatsConfig;
use discord_types::{AccessConfig, DmPolicy, GuildPolicy};
use serde::{Deserialize, Serialize};
use std::fs;

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

    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self> {
        let bot_token = std::env::var("DISCORD_BOT_TOKEN").context("DISCORD_BOT_TOKEN not set")?;

        let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "localhost:4222".to_string());

        let prefix = std::env::var("DISCORD_PREFIX").unwrap_or_else(|_| "prod".to_string());

        let guild_policy = match std::env::var("DISCORD_GUILD_POLICY")
            .unwrap_or_else(|_| "allowlist".to_string())
            .to_lowercase()
            .as_str()
        {
            "open" => GuildPolicy::Open,
            "disabled" => GuildPolicy::Disabled,
            _ => GuildPolicy::Allowlist,
        };

        let guild_allowlist =
            parse_id_list(&std::env::var("DISCORD_GUILD_ALLOWLIST").unwrap_or_default());

        let dm_policy = match std::env::var("DISCORD_DM_POLICY")
            .unwrap_or_else(|_| "allowlist".to_string())
            .to_lowercase()
            .as_str()
        {
            "open" => DmPolicy::Open,
            "disabled" => DmPolicy::Disabled,
            _ => DmPolicy::Allowlist,
        };

        let user_allowlist =
            parse_id_list(&std::env::var("DISCORD_USER_ALLOWLIST").unwrap_or_default());

        let admin_users = parse_id_list(&std::env::var("DISCORD_ADMIN_USERS").unwrap_or_default());

        let channel_allowlist =
            parse_id_list(&std::env::var("DISCORD_CHANNEL_ALLOWLIST").unwrap_or_default());

        let require_mention = std::env::var("DISCORD_REQUIRE_MENTION")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase()
            == "true";

        Ok(Config {
            discord: DiscordBotConfig {
                bot_token,
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
