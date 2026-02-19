//! Configuration management for discord-bot

#[path = "config_tests.rs"]
mod config_tests;

use anyhow::{Context, Result};
use discord_nats::NatsConfig;
use discord_types::{AccessConfig, DmPolicy, GuildPolicy};
use serde::{Deserialize, Serialize};
use std::fs;
use trogon_std::env::{ReadEnv, SystemEnv};

/// Complete bot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub discord: DiscordBotConfig,
    pub nats: NatsConfig,
}

/// Controls which reaction events are forwarded to NATS.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReactionMode {
    /// Publish no reaction events
    Off,
    /// Only publish reactions to messages sent by the bot itself
    #[default]
    Own,
    /// Publish all reaction events (default)
    All,
    /// Only publish reactions from users in `reaction_allowlist`
    Allowlist,
}

/// Controls when reply references are added to outbound messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyToMode {
    /// Never add a reply reference
    Off,
    /// Add a reply reference only to the first chunk (default)
    #[default]
    First,
    /// Add a reply reference to every chunk
    All,
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
    /// Controls which reaction events are forwarded to NATS
    #[serde(default)]
    pub reaction_mode: ReactionMode,
    /// User IDs allowed to trigger reaction events when `reaction_mode = allowlist`
    #[serde(default)]
    pub reaction_allowlist: Vec<u64>,
    /// Optional emoji to react with when a message is received and will be processed.
    /// Use a unicode emoji (e.g. "👀") or omit to disable.
    #[serde(default)]
    pub ack_reaction: Option<String>,
    /// Whether to process messages sent by other bots (default: false)
    #[serde(default)]
    pub allow_bots: bool,
    /// Optional string prepended to every outbound message sent by the bot
    #[serde(default)]
    pub response_prefix: Option<String>,
    /// Controls when reply references are added to outbound messages
    #[serde(default)]
    pub reply_to_mode: ReplyToMode,
    /// Maximum number of lines per outbound message chunk (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_lines_per_message: Option<usize>,
    /// Enable PluralKit member identity resolution for proxied messages
    #[serde(default)]
    pub pluralkit_enabled: bool,
    /// PluralKit API token (optional, improves rate limits)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluralkit_token: Option<String>,
    /// Allow messages from explicitly listed group DM channels
    #[serde(default)]
    pub dm_group_enabled: bool,
    /// Group DM channel IDs the bot should respond in (empty = all group DMs)
    #[serde(default)]
    pub dm_group_channels: Vec<u64>,
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
        let bot_token = env
            .var("DISCORD_BOT_TOKEN")
            .context("DISCORD_BOT_TOKEN not set")?;

        let nats_url = env
            .var("NATS_URL")
            .unwrap_or_else(|_| "localhost:4222".to_string());

        let prefix = env
            .var("DISCORD_PREFIX")
            .unwrap_or_else(|_| "prod".to_string());

        let guild_policy = match env
            .var("DISCORD_GUILD_POLICY")
            .unwrap_or_else(|_| "allowlist".to_string())
            .to_lowercase()
            .as_str()
        {
            "open" => GuildPolicy::Open,
            "disabled" => GuildPolicy::Disabled,
            _ => GuildPolicy::Allowlist,
        };

        let guild_allowlist =
            parse_id_list(&env.var("DISCORD_GUILD_ALLOWLIST").unwrap_or_default());

        let dm_policy = match env
            .var("DISCORD_DM_POLICY")
            .unwrap_or_else(|_| "allowlist".to_string())
            .to_lowercase()
            .as_str()
        {
            "open" => DmPolicy::Open,
            "pairing" => DmPolicy::Pairing,
            "disabled" => DmPolicy::Disabled,
            _ => DmPolicy::Allowlist,
        };

        let user_allowlist = parse_id_list(&env.var("DISCORD_USER_ALLOWLIST").unwrap_or_default());

        let admin_users = parse_id_list(&env.var("DISCORD_ADMIN_USERS").unwrap_or_default());

        let channel_allowlist =
            parse_id_list(&env.var("DISCORD_CHANNEL_ALLOWLIST").unwrap_or_default());

        let require_mention = env
            .var("DISCORD_REQUIRE_MENTION")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase()
            == "true";

        let presence_enabled = env
            .var("DISCORD_BRIDGE_PRESENCE")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase()
            == "true";

        let guild_commands_guild_id = env
            .var("DISCORD_GUILD_COMMANDS_GUILD_ID")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());

        let reaction_mode = match env
            .var("DISCORD_REACTION_MODE")
            .unwrap_or_else(|_| "own".to_string())
            .to_lowercase()
            .as_str()
        {
            "off" => ReactionMode::Off,
            "all" => ReactionMode::All,
            "allowlist" => ReactionMode::Allowlist,
            _ => ReactionMode::Own,
        };

        let reaction_allowlist =
            parse_id_list(&env.var("DISCORD_REACTION_ALLOWLIST").unwrap_or_default());

        let ack_reaction = env
            .var("DISCORD_ACK_REACTION")
            .ok()
            .filter(|s| !s.is_empty());

        let allow_bots = env
            .var("DISCORD_ALLOW_BOTS")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase()
            == "true";

        let response_prefix = env
            .var("DISCORD_RESPONSE_PREFIX")
            .ok()
            .filter(|s| !s.is_empty());

        let reply_to_mode = match env
            .var("DISCORD_REPLY_TO_MODE")
            .unwrap_or_else(|_| "first".to_string())
            .to_lowercase()
            .as_str()
        {
            "off" => ReplyToMode::Off,
            "all" => ReplyToMode::All,
            _ => ReplyToMode::First,
        };

        let max_lines_per_message = env
            .var("DISCORD_MAX_LINES_PER_MESSAGE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok());

        let pluralkit_enabled = env
            .var("DISCORD_PLURALKIT_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase()
            == "true";

        let pluralkit_token = env
            .var("DISCORD_PLURALKIT_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());

        let dm_group_enabled = env
            .var("DISCORD_DM_GROUP_ENABLED")
            .unwrap_or_else(|_| "false".to_string())
            .to_lowercase()
            == "true";

        let dm_group_channels =
            parse_id_list(&env.var("DISCORD_DM_GROUP_CHANNELS").unwrap_or_default());

        Ok(Config {
            discord: DiscordBotConfig {
                bot_token,
                presence_enabled,
                guild_commands_guild_id,
                reaction_mode,
                reaction_allowlist,
                ack_reaction,
                allow_bots,
                response_prefix,
                reply_to_mode,
                max_lines_per_message,
                pluralkit_enabled,
                pluralkit_token,
                dm_group_enabled,
                dm_group_channels,
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
