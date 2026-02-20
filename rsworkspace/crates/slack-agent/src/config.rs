use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

/// How the agent threads replies.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplyToMode {
    /// Never thread replies.
    Off,
    /// Thread only the first reply; subsequent replies continue in that thread.
    First,
    /// Always thread replies to the original message.
    All,
}

impl ReplyToMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "first" => Self::First,
            "all" => Self::All,
            _ => Self::Off,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlackAgentConfig {
    pub nats: NatsConfig,
    /// Controls threading behaviour. Read from SLACK_REPLY_TO_MODE env var.
    /// Values: "off" (default), "first", "all".
    pub reply_to_mode: ReplyToMode,
    /// Emoji shortcode (without colons) to add while processing a message,
    /// removed once a response is published. Mirrors OpenClaw `ackReaction`.
    /// Example: "eyes". Leave unset to disable.
    pub ack_reaction: Option<String>,

    // ── Claude / Anthropic ────────────────────────────────────────────────
    /// Anthropic API key. Required for AI responses.
    pub anthropic_api_key: Option<String>,
    /// Claude model ID. Default: "claude-sonnet-4-6".
    pub claude_model: String,
    /// Maximum output tokens per response. Default: 8192.
    pub claude_max_tokens: u32,
    /// Optional system prompt injected at the start of every conversation.
    pub claude_system_prompt: Option<String>,
    /// Path to a file whose contents become the system prompt.
    /// Takes precedence over `claude_system_prompt` when both are set.
    pub claude_system_prompt_file: Option<String>,
    /// Maximum conversation history messages retained per session. Default: 40.
    pub claude_max_history: usize,

    // ── Infra ─────────────────────────────────────────────────────────────
    /// Port for the HTTP health check endpoint. Default: 8081.
    pub health_port: u16,
}

impl SlackAgentConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let reply_to_mode = env
            .var("SLACK_REPLY_TO_MODE")
            .map(|v| ReplyToMode::from_str(&v))
            .unwrap_or(ReplyToMode::Off);

        let ack_reaction = env.var("SLACK_ACK_REACTION").ok().filter(|v| !v.is_empty());

        let anthropic_api_key = env.var("ANTHROPIC_API_KEY").ok().filter(|v| !v.is_empty());

        let claude_model = env
            .var("CLAUDE_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());

        let claude_max_tokens = env
            .var("CLAUDE_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8192);

        let claude_system_prompt = env
            .var("CLAUDE_SYSTEM_PROMPT")
            .ok()
            .filter(|v| !v.is_empty());

        let claude_system_prompt_file = env
            .var("CLAUDE_SYSTEM_PROMPT_FILE")
            .ok()
            .filter(|v| !v.is_empty());

        let claude_max_history = env
            .var("CLAUDE_MAX_HISTORY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40);

        let health_port = env
            .var("HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8081);

        Self {
            nats: NatsConfig::from_env(env),
            reply_to_mode,
            ack_reaction,
            anthropic_api_key,
            claude_model,
            claude_max_tokens,
            claude_system_prompt,
            claude_system_prompt_file,
            claude_max_history,
            health_port,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    #[test]
    fn reply_to_mode_from_str() {
        assert_eq!(ReplyToMode::from_str("first"), ReplyToMode::First);
        assert_eq!(ReplyToMode::from_str("all"), ReplyToMode::All);
        assert_eq!(ReplyToMode::from_str("off"), ReplyToMode::Off);
        assert_eq!(ReplyToMode::from_str(""), ReplyToMode::Off);
        assert_eq!(ReplyToMode::from_str("unknown"), ReplyToMode::Off);
    }

    #[test]
    fn from_env_defaults() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.reply_to_mode, ReplyToMode::Off);
        assert!(config.ack_reaction.is_none());
        assert!(config.anthropic_api_key.is_none());
        assert_eq!(config.claude_model, "claude-sonnet-4-6");
        assert_eq!(config.claude_max_tokens, 8192);
        assert_eq!(config.claude_max_history, 40);
        assert_eq!(config.health_port, 8081);
    }

    #[test]
    fn from_env_custom_values() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env.set("SLACK_REPLY_TO_MODE", "first");
        env.set("SLACK_ACK_REACTION", "eyes");
        env.set("ANTHROPIC_API_KEY", "sk-ant-test");
        env.set("CLAUDE_MODEL", "claude-opus-4-6");
        env.set("CLAUDE_MAX_TOKENS", "4096");
        env.set("CLAUDE_MAX_HISTORY", "20");
        env.set("HEALTH_PORT", "9090");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.reply_to_mode, ReplyToMode::First);
        assert_eq!(config.ack_reaction.as_deref(), Some("eyes"));
        assert_eq!(config.anthropic_api_key.as_deref(), Some("sk-ant-test"));
        assert_eq!(config.claude_model, "claude-opus-4-6");
        assert_eq!(config.claude_max_tokens, 4096);
        assert_eq!(config.claude_max_history, 20);
        assert_eq!(config.health_port, 9090);
    }

    #[test]
    fn system_prompt_file_takes_precedence() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env.set("CLAUDE_SYSTEM_PROMPT", "inline prompt");
        env.set("CLAUDE_SYSTEM_PROMPT_FILE", "/tmp/prompt.txt");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(
            config.claude_system_prompt_file.as_deref(),
            Some("/tmp/prompt.txt")
        );
        // Both are stored; main.rs resolves precedence at runtime.
        assert_eq!(
            config.claude_system_prompt.as_deref(),
            Some("inline prompt")
        );
    }

    #[test]
    fn empty_system_prompt_file_treated_as_none() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env.set("CLAUDE_SYSTEM_PROMPT_FILE", "");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.claude_system_prompt_file.is_none());
    }

    #[test]
    fn empty_ack_reaction_treated_as_none() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env.set("SLACK_ACK_REACTION", "");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.ack_reaction.is_none());
    }
}
