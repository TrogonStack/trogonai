/// Runtime configuration for the openrouter-runner binary.
///
/// Reads environment variables with sensible defaults and provides a single
/// place to unit-test the parsing logic without touching `main`.
pub struct RunnerConfig {
    pub nats_url: String,
    pub prefix: String,
    pub default_model: String,
    /// `None` when `OPENROUTER_API_KEY` is absent or empty.
    pub api_key: Option<String>,
    pub agent_type: String,
    /// Kept as a raw string for logging; the agent re-parses it internally.
    pub models_str: String,
    pub max_history_messages: usize,
    pub session_ttl_secs: u64,
    pub prompt_timeout_secs: u64,
    pub system_prompt_set: bool,
}

impl RunnerConfig {
    pub fn from_env() -> Self {
        let nats_url = std::env::var("NATS_URL")
            .unwrap_or_else(|_| "nats://localhost:4222".to_string());

        let prefix = std::env::var("ACP_PREFIX")
            .unwrap_or_else(|_| "acp".to_string());

        let default_model = std::env::var("OPENROUTER_DEFAULT_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4-6".to_string());

        let api_key = std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());

        let agent_type = std::env::var("AGENT_TYPE")
            .unwrap_or_else(|_| "openrouter".to_string());

        let models_str = std::env::var("OPENROUTER_MODELS").unwrap_or_else(|_| {
            "anthropic/claude-sonnet-4-6:Claude Sonnet 4.6,openai/gpt-4o:GPT-4o,google/gemini-pro-1.5:Gemini Pro 1.5".to_string()
        });

        let max_history_messages = std::env::var("OPENROUTER_MAX_HISTORY_MESSAGES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(20);

        let session_ttl_secs = std::env::var("OPENROUTER_SESSION_TTL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(7 * 24 * 3600);

        let prompt_timeout_secs = std::env::var("OPENROUTER_PROMPT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(300);

        let system_prompt_set = std::env::var("OPENROUTER_SYSTEM_PROMPT").is_ok();

        Self {
            nats_url,
            prefix,
            default_model,
            api_key,
            agent_type,
            models_str,
            max_history_messages,
            session_ttl_secs,
            prompt_timeout_secs,
            system_prompt_set,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_runner_env() {
        for var in &[
            "NATS_URL",
            "ACP_PREFIX",
            "OPENROUTER_DEFAULT_MODEL",
            "OPENROUTER_API_KEY",
            "AGENT_TYPE",
            "OPENROUTER_MODELS",
            "OPENROUTER_MAX_HISTORY_MESSAGES",
            "OPENROUTER_SESSION_TTL_SECS",
            "OPENROUTER_PROMPT_TIMEOUT_SECS",
            "OPENROUTER_SYSTEM_PROMPT",
        ] {
            unsafe { std::env::remove_var(var); }
        }
    }

    #[test]
    fn defaults_when_no_env_vars_set() {
        clear_runner_env();
        let cfg = RunnerConfig::from_env();
        assert_eq!(cfg.nats_url, "nats://localhost:4222");
        assert_eq!(cfg.prefix, "acp");
        assert_eq!(cfg.default_model, "anthropic/claude-sonnet-4-6");
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.agent_type, "openrouter");
        assert_eq!(cfg.max_history_messages, 20);
        assert_eq!(cfg.session_ttl_secs, 7 * 24 * 3600);
        assert_eq!(cfg.prompt_timeout_secs, 300);
        assert!(!cfg.system_prompt_set);
    }

    #[test]
    fn custom_string_values_are_read() {
        clear_runner_env();
        unsafe {
            std::env::set_var("NATS_URL", "nats://my-server:4222");
            std::env::set_var("ACP_PREFIX", "prod");
            std::env::set_var("OPENROUTER_DEFAULT_MODEL", "openai/gpt-4o");
            std::env::set_var("AGENT_TYPE", "my-agent");
        }
        let cfg = RunnerConfig::from_env();
        clear_runner_env();
        assert_eq!(cfg.nats_url, "nats://my-server:4222");
        assert_eq!(cfg.prefix, "prod");
        assert_eq!(cfg.default_model, "openai/gpt-4o");
        assert_eq!(cfg.agent_type, "my-agent");
    }

    #[test]
    fn api_key_present_and_non_empty_is_some() {
        clear_runner_env();
        unsafe { std::env::set_var("OPENROUTER_API_KEY", "sk-test-123"); }
        let cfg = RunnerConfig::from_env();
        clear_runner_env();
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test-123"));
    }

    #[test]
    fn empty_api_key_becomes_none() {
        clear_runner_env();
        unsafe { std::env::set_var("OPENROUTER_API_KEY", ""); }
        let cfg = RunnerConfig::from_env();
        clear_runner_env();
        assert!(cfg.api_key.is_none(), "empty API key must be treated as absent");
    }

    #[test]
    fn system_prompt_set_flag_reflects_env() {
        clear_runner_env();
        assert!(!RunnerConfig::from_env().system_prompt_set);
        unsafe { std::env::set_var("OPENROUTER_SYSTEM_PROMPT", ""); }
        assert!(RunnerConfig::from_env().system_prompt_set, "flag must be true even for empty value");
        clear_runner_env();
    }

    #[test]
    fn numeric_env_vars_parsed_correctly() {
        clear_runner_env();
        unsafe {
            std::env::set_var("OPENROUTER_MAX_HISTORY_MESSAGES", "50");
            std::env::set_var("OPENROUTER_SESSION_TTL_SECS", "3600");
            std::env::set_var("OPENROUTER_PROMPT_TIMEOUT_SECS", "120");
        }
        let cfg = RunnerConfig::from_env();
        clear_runner_env();
        assert_eq!(cfg.max_history_messages, 50);
        assert_eq!(cfg.session_ttl_secs, 3600);
        assert_eq!(cfg.prompt_timeout_secs, 120);
    }

    #[test]
    fn zero_numeric_values_fall_back_to_defaults() {
        clear_runner_env();
        unsafe {
            std::env::set_var("OPENROUTER_MAX_HISTORY_MESSAGES", "0");
            std::env::set_var("OPENROUTER_SESSION_TTL_SECS", "0");
            std::env::set_var("OPENROUTER_PROMPT_TIMEOUT_SECS", "0");
        }
        let cfg = RunnerConfig::from_env();
        clear_runner_env();
        assert_eq!(cfg.max_history_messages, 20, "zero must fall back to default");
        assert_eq!(cfg.session_ttl_secs, 7 * 24 * 3600, "zero must fall back to default");
        assert_eq!(cfg.prompt_timeout_secs, 300, "zero must fall back to default");
    }

    #[test]
    fn non_numeric_env_vars_fall_back_to_defaults() {
        clear_runner_env();
        unsafe {
            std::env::set_var("OPENROUTER_MAX_HISTORY_MESSAGES", "not-a-number");
            std::env::set_var("OPENROUTER_SESSION_TTL_SECS", "one-week");
            std::env::set_var("OPENROUTER_PROMPT_TIMEOUT_SECS", "five-minutes");
        }
        let cfg = RunnerConfig::from_env();
        clear_runner_env();
        assert_eq!(cfg.max_history_messages, 20);
        assert_eq!(cfg.session_ttl_secs, 7 * 24 * 3600);
        assert_eq!(cfg.prompt_timeout_secs, 300);
    }

    #[test]
    fn models_str_is_read_verbatim() {
        clear_runner_env();
        unsafe { std::env::set_var("OPENROUTER_MODELS", "x/y:Label"); }
        let cfg = RunnerConfig::from_env();
        clear_runner_env();
        assert_eq!(cfg.models_str, "x/y:Label");
    }
}
