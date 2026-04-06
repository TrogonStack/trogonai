#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use telegram_types::AccessConfig;
use trogon_std::env::ReadEnv;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub telegram: TelegramBotConfig,
    #[serde(default = "default_prefix")]
    pub prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramBotConfig {
    #[serde(default = "default_bot_token")]
    pub bot_token: String,
    #[serde(default)]
    pub access: AccessConfig,
    #[serde(default)]
    pub features: FeatureConfig,
    #[serde(default)]
    pub limits: LimitConfig,
    #[serde(default)]
    pub update_mode: UpdateModeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureConfig {
    #[serde(default = "default_true")]
    pub inline_buttons: bool,
    #[serde(default = "default_streaming")]
    pub streaming: StreamingMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StreamingMode {
    Disabled,
    Partial,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum UpdateModeConfig {
    Polling {
        #[serde(default = "default_polling_timeout")]
        timeout: u32,
        #[serde(default = "default_polling_limit")]
        limit: u8,
    },
    Webhook {
        url: String,
        #[serde(default = "default_webhook_port")]
        port: u16,
        #[serde(default = "default_webhook_path")]
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        secret_token: Option<String>,
        #[serde(default = "default_webhook_bind")]
        bind_address: String,
        #[serde(default = "default_max_connections")]
        max_connections: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitConfig {
    #[serde(default = "default_text_chunk_limit")]
    pub text_chunk_limit: usize,
    #[serde(default = "default_media_max_mb")]
    pub media_max_mb: u64,
    #[serde(default = "default_history_limit")]
    pub history_limit: usize,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_messages_per_minute: u32,
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path))?;

        Ok(config)
    }

    pub fn from_env<E: ReadEnv>(env: &E) -> Result<Self> {
        let bot_token = env
            .var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| anyhow::anyhow!("TELEGRAM_BOT_TOKEN not set"))?;

        let prefix = env
            .var("TELEGRAM_PREFIX")
            .unwrap_or_else(|_| "prod".to_string());

        let update_mode = if let Ok(webhook_url) = env.var("TELEGRAM_WEBHOOK_URL") {
            let port = env
                .var("TELEGRAM_WEBHOOK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8443);
            let path = env
                .var("TELEGRAM_WEBHOOK_PATH")
                .unwrap_or_else(|_| "/webhook".to_string());
            let secret_token = env.var("TELEGRAM_WEBHOOK_SECRET").ok();

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
            prefix,
        })
    }
}

fn default_bot_token() -> String {
    std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default()
}

fn default_prefix() -> String {
    "prod".to_string()
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
