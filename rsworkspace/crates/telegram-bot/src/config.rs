//! Configuration management for telegram-bot

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use telegram_nats::NatsConfig;
use telegram_types::AccessConfig;

/// Complete bot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub telegram: TelegramBotConfig,
    pub nats: NatsConfig,
}

/// Telegram bot specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramBotConfig {
    /// Bot token from BotFather
    #[serde(default = "default_bot_token")]
    pub bot_token: String,
    /// Access control configuration
    #[serde(default)]
    pub access: AccessConfig,
    /// Feature flags
    #[serde(default)]
    pub features: FeatureConfig,
    /// Rate limiting configuration
    #[serde(default)]
    pub limits: LimitConfig,
}

/// Feature flags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureConfig {
    /// Enable inline buttons
    #[serde(default = "default_true")]
    pub inline_buttons: bool,
    /// Enable streaming (partial message updates)
    #[serde(default = "default_streaming")]
    pub streaming: StreamingMode,
}

/// Streaming mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StreamingMode {
    Disabled,
    Partial,
    Full,
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitConfig {
    /// Max text chunk size (Telegram limit: 4096)
    #[serde(default = "default_text_chunk_limit")]
    pub text_chunk_limit: usize,
    /// Max media file size in MB
    #[serde(default = "default_media_max_mb")]
    pub media_max_mb: u64,
    /// Message history limit per session
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    /// Rate limit: messages per minute
    #[serde(default = "default_rate_limit")]
    pub rate_limit_messages_per_minute: u32,
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
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
            .context("TELEGRAM_BOT_TOKEN not set")?;

        let nats_url = std::env::var("NATS_URL")
            .unwrap_or_else(|_| "localhost:4222".to_string());

        let prefix = std::env::var("TELEGRAM_PREFIX")
            .unwrap_or_else(|_| "prod".to_string());

        Ok(Config {
            telegram: TelegramBotConfig {
                bot_token,
                access: AccessConfig::default(),
                features: FeatureConfig::default(),
                limits: LimitConfig::default(),
            },
            nats: NatsConfig::from_url(nats_url, prefix),
        })
    }
}

fn default_bot_token() -> String {
    std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default()
}

fn default_true() -> bool {
    true
}

fn default_streaming() -> StreamingMode {
    StreamingMode::Partial
}

fn default_text_chunk_limit() -> usize {
    4096
}

fn default_media_max_mb() -> u64 {
    50
}

fn default_history_limit() -> usize {
    100
}

fn default_rate_limit() -> u32 {
    20
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            inline_buttons: true,
            streaming: StreamingMode::Partial,
        }
    }
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            text_chunk_limit: default_text_chunk_limit(),
            media_max_mb: default_media_max_mb(),
            history_limit: default_history_limit(),
            rate_limit_messages_per_minute: default_rate_limit(),
        }
    }
}
