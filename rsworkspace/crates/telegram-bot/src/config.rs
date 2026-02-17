//! Configuration management for telegram-bot

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;

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
    /// Update mode: polling or webhook
    #[serde(default)]
    pub update_mode: UpdateModeConfig,
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

/// Update mode configuration (webhook or polling)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum UpdateModeConfig {
    /// Long polling mode (default)
    Polling {
        /// Timeout in seconds for long polling
        #[serde(default = "default_polling_timeout")]
        timeout: u32,
        /// Maximum number of updates to fetch at once
        #[serde(default = "default_polling_limit")]
        limit: u8,
    },
    /// Webhook mode
    Webhook {
        /// Public URL where Telegram will send updates
        /// Example: "https://example.com/webhook"
        url: String,
        /// Port to listen on for webhook requests
        #[serde(default = "default_webhook_port")]
        port: u16,
        /// Path where webhook will listen (default: "/webhook")
        #[serde(default = "default_webhook_path")]
        path: String,
        /// Optional secret token to validate requests
        #[serde(skip_serializing_if = "Option::is_none")]
        secret_token: Option<String>,
        /// IP address to bind to (default: 0.0.0.0)
        #[serde(default = "default_webhook_bind")]
        bind_address: String,
        /// Maximum allowed number of simultaneous HTTPS connections to the webhook
        #[serde(default = "default_max_connections")]
        max_connections: u8,
    },
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

        // Check for webhook mode from environment
        let update_mode = if let Ok(webhook_url) = std::env::var("TELEGRAM_WEBHOOK_URL") {
            let port = std::env::var("TELEGRAM_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8443);
            let path = std::env::var("TELEGRAM_WEBHOOK_PATH")
                .unwrap_or_else(|_| "/webhook".to_string());
            let secret_token = std::env::var("TELEGRAM_WEBHOOK_SECRET").ok();

            UpdateModeConfig::Webhook {
                url: webhook_url,
                port,
                path,
                secret_token,
                bind_address: "0.0.0.0".to_string(),
                max_connections: 40,
            }
        } else {
            UpdateModeConfig::default()
        };

        Ok(Config {
            telegram: TelegramBotConfig {
                bot_token,
                access: AccessConfig::default(),
                features: FeatureConfig::default(),
                limits: LimitConfig::default(),
                update_mode,
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

fn default_polling_timeout() -> u32 {
    30
}

fn default_polling_limit() -> u8 {
    100
}

fn default_webhook_port() -> u16 {
    8443
}

fn default_webhook_path() -> String {
    "/webhook".to_string()
}

fn default_webhook_bind() -> String {
    "0.0.0.0".to_string()
}

fn default_max_connections() -> u8 {
    40
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

impl Default for UpdateModeConfig {
    fn default() -> Self {
        UpdateModeConfig::Polling {
            timeout: default_polling_timeout(),
            limit: default_polling_limit(),
        }
    }
}
