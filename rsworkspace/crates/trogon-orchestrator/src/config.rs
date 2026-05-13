use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::provider::{OrchestratorAuthStyle, OrchestratorLlmConfig};

const DEFAULT_PORT: u16 = 8087;
const DEFAULT_LLM_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_LLM_MODEL: &str = "claude-sonnet-4-6";
const DEFAULT_LLM_MAX_TOKENS: u32 = 8_192;

/// Configuration for the trogon-orchestrator service.
///
/// | Variable                         | Default                                 |
/// |----------------------------------|-----------------------------------------|
/// | `ORCHESTRATOR_LLM_URL`           | `https://api.anthropic.com/v1/messages` |
/// | `ORCHESTRATOR_LLM_API_KEY`       | *(required)*                            |
/// | `ORCHESTRATOR_LLM_AUTH_STYLE`    | `xapikey` (or `bearer`)                 |
/// | `ORCHESTRATOR_LLM_MODEL`         | `claude-sonnet-4-6`                     |
/// | `ORCHESTRATOR_LLM_MAX_TOKENS`    | `8192`                                  |
/// | `ORCHESTRATOR_PORT`              | `8087`                                  |
/// | `ORCHESTRATOR_AGENT_TIMEOUT_SECS`| `60`                                    |
/// | Standard `NATS_*` variables for NATS connection                       |
pub struct OrchestratorConfig {
    pub llm: OrchestratorLlmConfig,
    pub port: u16,
    pub agent_timeout_secs: u64,
    pub nats: NatsConfig,
}

impl OrchestratorConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let auth_style = match env
            .var("ORCHESTRATOR_LLM_AUTH_STYLE")
            .as_deref()
            .unwrap_or("xapikey")
        {
            "bearer" => OrchestratorAuthStyle::Bearer,
            _ => OrchestratorAuthStyle::XApiKey,
        };

        Self {
            llm: OrchestratorLlmConfig {
                api_url: env
                    .var("ORCHESTRATOR_LLM_URL")
                    .unwrap_or_else(|_| DEFAULT_LLM_URL.to_string()),
                api_key: env.var("ORCHESTRATOR_LLM_API_KEY").unwrap_or_default(),
                auth_style,
                model: env
                    .var("ORCHESTRATOR_LLM_MODEL")
                    .unwrap_or_else(|_| DEFAULT_LLM_MODEL.to_string()),
                max_tokens: env
                    .var("ORCHESTRATOR_LLM_MAX_TOKENS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_LLM_MAX_TOKENS),
            },
            port: env
                .var("ORCHESTRATOR_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            agent_timeout_secs: env
                .var("ORCHESTRATOR_AGENT_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
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
        let cfg = OrchestratorConfig::from_env(&env);
        assert_eq!(cfg.llm.api_url, DEFAULT_LLM_URL);
        assert_eq!(cfg.llm.model, DEFAULT_LLM_MODEL);
        assert_eq!(cfg.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.agent_timeout_secs, 60);
    }

    #[test]
    fn reads_all_env_vars() {
        let env = InMemoryEnv::new();
        env.set("ORCHESTRATOR_LLM_URL", "http://proxy/v1/messages");
        env.set("ORCHESTRATOR_LLM_API_KEY", "secret");
        env.set("ORCHESTRATOR_LLM_AUTH_STYLE", "bearer");
        env.set("ORCHESTRATOR_LLM_MODEL", "claude-opus-4-7");
        env.set("ORCHESTRATOR_LLM_MAX_TOKENS", "16384");
        env.set("ORCHESTRATOR_PORT", "9090");
        env.set("ORCHESTRATOR_AGENT_TIMEOUT_SECS", "120");

        let cfg = OrchestratorConfig::from_env(&env);
        assert_eq!(cfg.llm.api_url, "http://proxy/v1/messages");
        assert_eq!(cfg.llm.api_key, "secret");
        assert!(matches!(cfg.llm.auth_style, OrchestratorAuthStyle::Bearer));
        assert_eq!(cfg.llm.model, "claude-opus-4-7");
        assert_eq!(cfg.llm.max_tokens, 16384);
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.agent_timeout_secs, 120);
    }

    #[test]
    fn invalid_numeric_values_fall_back_to_defaults() {
        let env = InMemoryEnv::new();
        env.set("ORCHESTRATOR_LLM_MAX_TOKENS", "nope");
        env.set("ORCHESTRATOR_PORT", "nope");
        env.set("ORCHESTRATOR_AGENT_TIMEOUT_SECS", "nope");
        let cfg = OrchestratorConfig::from_env(&env);
        assert_eq!(cfg.llm.max_tokens, DEFAULT_LLM_MAX_TOKENS);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.agent_timeout_secs, 60);
    }
}
