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

        let ack_reaction = env
            .var("SLACK_ACK_REACTION")
            .ok()
            .filter(|v| !v.is_empty());

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
            claude_max_history,
            health_port,
        }
    }
}
