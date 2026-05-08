use std::time::Duration;

use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::provider::{MemoryAuthStyle, MemoryLlmConfig};

const DEFAULT_PORT: u16 = 8085;
const DEFAULT_CONSUMER_NAME: &str = "trogon-memory";
const DEFAULT_LLM_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_LLM_MODEL: &str = "claude-haiku-4-5-20251001";
const DEFAULT_LLM_MAX_TOKENS: u32 = 4_096;
const DEFAULT_LLM_TIMEOUT_SECS: u64 = 30;

/// Configuration for the trogon-memory dreaming service.
///
/// | Variable                    | Default                                        |
/// |-----------------------------|------------------------------------------------|
/// | `MEMORY_CONSUMER_NAME`      | `trogon-memory`                                |
/// | `MEMORY_LLM_URL`            | `https://api.anthropic.com/v1/messages`        |
/// | `MEMORY_LLM_API_KEY`        | *(required)*                                   |
/// | `MEMORY_LLM_AUTH_STYLE`     | `xapikey` (or `bearer`)                        |
/// | `MEMORY_LLM_MODEL`          | `claude-haiku-4-5-20251001`                    |
/// | `MEMORY_LLM_MAX_TOKENS`     | `4096`                                         |
/// | `MEMORY_PORT`               | `8085`                                         |
/// | Standard `NATS_*` variables for NATS connection                    |
pub struct DreamingConfig {
    pub consumer_name: String,
    pub llm: MemoryLlmConfig,
    pub llm_timeout: Duration,
    pub port: u16,
    pub nats: NatsConfig,
}

impl DreamingConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let auth_style = match env
            .var("MEMORY_LLM_AUTH_STYLE")
            .as_deref()
            .unwrap_or("xapikey")
        {
            "bearer" => MemoryAuthStyle::Bearer,
            _ => MemoryAuthStyle::XApiKey,
        };

        Self {
            consumer_name: env
                .var("MEMORY_CONSUMER_NAME")
                .unwrap_or_else(|_| DEFAULT_CONSUMER_NAME.to_string()),
            llm: MemoryLlmConfig {
                api_url: env
                    .var("MEMORY_LLM_URL")
                    .unwrap_or_else(|_| DEFAULT_LLM_URL.to_string()),
                api_key: env.var("MEMORY_LLM_API_KEY").unwrap_or_default(),
                auth_style,
                model: env
                    .var("MEMORY_LLM_MODEL")
                    .unwrap_or_else(|_| DEFAULT_LLM_MODEL.to_string()),
                max_tokens: env
                    .var("MEMORY_LLM_MAX_TOKENS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_LLM_MAX_TOKENS),
            },
            llm_timeout: Duration::from_secs(
                env.var("MEMORY_LLM_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_LLM_TIMEOUT_SECS),
            ),
            port: env
                .var("MEMORY_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            nats: NatsConfig::from_env(env),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn defaults_when_no_env_vars() {
        let env = InMemoryEnv::new();
        let cfg = DreamingConfig::from_env(&env);
        assert_eq!(cfg.consumer_name, "trogon-memory");
        assert_eq!(cfg.llm.api_url, DEFAULT_LLM_URL);
        assert_eq!(cfg.llm.model, DEFAULT_LLM_MODEL);
        assert_eq!(cfg.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(cfg.port, DEFAULT_PORT);
    }

    #[test]
    fn reads_custom_env_vars() {
        let env = InMemoryEnv::new();
        env.set("MEMORY_CONSUMER_NAME", "my-dreamer");
        env.set("MEMORY_LLM_URL", "http://proxy/v1/messages");
        env.set("MEMORY_LLM_API_KEY", "secret");
        env.set("MEMORY_LLM_AUTH_STYLE", "bearer");
        env.set("MEMORY_LLM_MODEL", "claude-opus-4-7");
        env.set("MEMORY_LLM_MAX_TOKENS", "8192");
        env.set("MEMORY_PORT", "9000");

        let cfg = DreamingConfig::from_env(&env);
        assert_eq!(cfg.consumer_name, "my-dreamer");
        assert_eq!(cfg.llm.api_url, "http://proxy/v1/messages");
        assert_eq!(cfg.llm.api_key, "secret");
        assert!(matches!(cfg.llm.auth_style, MemoryAuthStyle::Bearer));
        assert_eq!(cfg.llm.model, "claude-opus-4-7");
        assert_eq!(cfg.llm.max_tokens, 8192);
        assert_eq!(cfg.port, 9000);
    }

    #[test]
    fn invalid_numeric_values_fall_back_to_defaults() {
        let env = InMemoryEnv::new();
        env.set("MEMORY_LLM_MAX_TOKENS", "nope");
        env.set("MEMORY_PORT", "nope");
        let cfg = DreamingConfig::from_env(&env);
        assert_eq!(cfg.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(cfg.port, DEFAULT_PORT);
    }
}
