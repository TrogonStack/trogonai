//! Configuration management for discord-bot

#[path = "config_tests.rs"]
mod config_tests;

use anyhow::{Context, Result};
use discord_nats::NatsConfig;
use discord_types::AccessConfig;
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

        Ok(Config {
            discord: DiscordBotConfig {
                bot_token,
                access: AccessConfig::default(),
            },
            nats: NatsConfig::from_url(nats_url, prefix),
        })
    }
}

fn default_bot_token() -> String {
    std::env::var("DISCORD_BOT_TOKEN").unwrap_or_default()
}
