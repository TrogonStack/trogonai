use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::provider::{EvalAuthStyle, EvalLlmConfig};

const DEFAULT_PORT: u16 = 8086;
const DEFAULT_CONSUMER_NAME: &str = "trogon-outcomes";
const DEFAULT_LLM_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_LLM_MODEL: &str = "claude-haiku-4-5-20251001";
const DEFAULT_LLM_MAX_TOKENS: u32 = 4_096;

/// Configuration for the trogon-outcomes evaluation service.
///
/// | Variable                     | Default                                     |
/// |------------------------------|---------------------------------------------|
/// | `OUTCOMES_CONSUMER_NAME`     | `trogon-outcomes`                           |
/// | `OUTCOMES_LLM_URL`           | `https://api.anthropic.com/v1/messages`     |
/// | `OUTCOMES_LLM_API_KEY`       | *(required)*                                |
/// | `OUTCOMES_LLM_AUTH_STYLE`    | `xapikey` (or `bearer`)                     |
/// | `OUTCOMES_LLM_MODEL`         | `claude-haiku-4-5-20251001`                 |
/// | `OUTCOMES_LLM_MAX_TOKENS`    | `4096`                                      |
/// | `OUTCOMES_PORT`              | `8086`                                      |
/// | Standard `NATS_*` variables for NATS connection                   |
pub struct OutcomesConfig {
    pub consumer_name: String,
    pub llm: EvalLlmConfig,
    pub port: u16,
    pub nats: NatsConfig,
}

impl OutcomesConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let auth_style = match env
            .var("OUTCOMES_LLM_AUTH_STYLE")
            .as_deref()
            .unwrap_or("xapikey")
        {
            "bearer" => EvalAuthStyle::Bearer,
            _ => EvalAuthStyle::XApiKey,
        };

        Self {
            consumer_name: env
                .var("OUTCOMES_CONSUMER_NAME")
                .unwrap_or_else(|_| DEFAULT_CONSUMER_NAME.to_string()),
            llm: EvalLlmConfig {
                api_url: env
                    .var("OUTCOMES_LLM_URL")
                    .unwrap_or_else(|_| DEFAULT_LLM_URL.to_string()),
                api_key: env.var("OUTCOMES_LLM_API_KEY").unwrap_or_default(),
                auth_style,
                model: env
                    .var("OUTCOMES_LLM_MODEL")
                    .unwrap_or_else(|_| DEFAULT_LLM_MODEL.to_string()),
                max_tokens: env
                    .var("OUTCOMES_LLM_MAX_TOKENS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_LLM_MAX_TOKENS),
            },
            port: env
                .var("OUTCOMES_PORT")
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
        let cfg = OutcomesConfig::from_env(&env);
        assert_eq!(cfg.consumer_name, "trogon-outcomes");
        assert_eq!(cfg.llm.api_url, DEFAULT_LLM_URL);
        assert_eq!(cfg.llm.model, DEFAULT_LLM_MODEL);
        assert_eq!(cfg.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(cfg.port, DEFAULT_PORT);
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("OUTCOMES_CONSUMER_NAME", "my-evaluator");
        env.set("OUTCOMES_LLM_URL", "http://proxy/v1/messages");
        env.set("OUTCOMES_LLM_API_KEY", "secret");
        env.set("OUTCOMES_LLM_AUTH_STYLE", "bearer");
        env.set("OUTCOMES_LLM_MODEL", "claude-opus-4-7");
        env.set("OUTCOMES_LLM_MAX_TOKENS", "8192");
        env.set("OUTCOMES_PORT", "9001");

        let cfg = OutcomesConfig::from_env(&env);
        assert_eq!(cfg.consumer_name, "my-evaluator");
        assert_eq!(cfg.llm.api_url, "http://proxy/v1/messages");
        assert_eq!(cfg.llm.api_key, "secret");
        assert!(matches!(cfg.llm.auth_style, EvalAuthStyle::Bearer));
        assert_eq!(cfg.llm.model, "claude-opus-4-7");
        assert_eq!(cfg.llm.max_tokens, 8192);
        assert_eq!(cfg.port, 9001);
    }

    #[test]
    fn invalid_numeric_values_fall_back_to_defaults() {
        let env = InMemoryEnv::new();
        env.set("OUTCOMES_LLM_MAX_TOKENS", "nope");
        env.set("OUTCOMES_PORT", "nope");
        let cfg = OutcomesConfig::from_env(&env);
        assert_eq!(cfg.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(cfg.port, DEFAULT_PORT);
    }
}
