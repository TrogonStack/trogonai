use std::path::Path;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;
use trogon_std::fs::ReadFile;

/// Controls who is allowed to send the bot direct messages.
#[derive(Debug, Clone, PartialEq)]
pub enum DmPolicy {
    /// Any Slack user may DM the bot (default).
    Open,
    /// All DMs are silently ignored.
    Disabled,
}

impl DmPolicy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "disabled" => Self::Disabled,
            _ => Self::Open,
        }
    }
}

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
    /// Maximum total character count of message content kept in history.
    /// `0` = disabled. Characters are used as a token proxy (chars ÷ 4 ≈ tokens).
    /// Read from `CLAUDE_MAX_HISTORY_CHARS`. Default: 0 (disabled).
    pub claude_max_history_chars: usize,

    // ── Infra ─────────────────────────────────────────────────────────────
    /// Port for the HTTP health check endpoint. Default: 8081.
    pub health_port: u16,
    /// Optional message posted to a channel when a new member joins.
    /// Controlled by SLACK_WELCOME_MESSAGE env var.
    pub welcome_message: Option<String>,

    // ── Access control ────────────────────────────────────────────────────────
    /// Controls who is allowed to DM the bot. Read from SLACK_DM_POLICY env var.
    /// Values: "open" (default), "disabled".
    pub dm_policy: DmPolicy,
    /// When non-empty, only messages from these channel IDs are processed.
    /// DMs bypass this check. Read from SLACK_CHANNEL_ALLOWLIST env var as a
    /// comma-separated list of channel IDs.
    pub channel_allowlist: Vec<String>,

    // ── Per-chat-type reply mode overrides ────────────────────────────────────
    /// When set, overrides reply_to_mode for DM sessions.
    pub reply_to_mode_dm: Option<ReplyToMode>,
    /// When set, overrides reply_to_mode for group sessions.
    pub reply_to_mode_group: Option<ReplyToMode>,

    // ── Thread history ────────────────────────────────────────────────────────
    /// Number of messages to load from the parent session when a new thread
    /// starts. Default: 0 (disabled). Mirrors OpenClaw thread.initialHistoryLimit.
    pub thread_initial_history_limit: usize,

    // ── Bot message filtering ─────────────────────────────────────────────────
    /// When true, messages from other bots are processed. Default: false.
    #[allow(dead_code)]
    pub allow_bots: bool,
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

        let claude_max_history_chars = env
            .var("CLAUDE_MAX_HISTORY_CHARS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let health_port = env
            .var("HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8081);

        let welcome_message = env
            .var("SLACK_WELCOME_MESSAGE")
            .ok()
            .filter(|v| !v.is_empty());

        let dm_policy = env
            .var("SLACK_DM_POLICY")
            .map(|v| DmPolicy::from_str(&v))
            .unwrap_or(DmPolicy::Open);

        let channel_allowlist = env
            .var("SLACK_CHANNEL_ALLOWLIST")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let reply_to_mode_dm = env
            .var("SLACK_REPLY_TO_MODE_DM")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| ReplyToMode::from_str(&v));

        let reply_to_mode_group = env
            .var("SLACK_REPLY_TO_MODE_GROUP")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| ReplyToMode::from_str(&v));

        let thread_initial_history_limit = env
            .var("THREAD_INITIAL_HISTORY_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let allow_bots = env
            .var("SLACK_ALLOW_BOTS")
            .ok()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

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
            claude_max_history_chars,
            health_port,
            welcome_message,
            dm_policy,
            channel_allowlist,
            reply_to_mode_dm,
            reply_to_mode_group,
            thread_initial_history_limit,
            allow_bots,
        }
    }

    /// Resolve the effective system prompt using the given filesystem abstraction.
    ///
    /// When `claude_system_prompt_file` is set, its contents (trimmed) take
    /// precedence over `claude_system_prompt`. Falls back to `claude_system_prompt`
    /// if the file is absent or unreadable.
    pub fn resolve_system_prompt<F: ReadFile>(&self, fs: &F) -> Option<String> {
        self.claude_system_prompt_file
            .as_deref()
            .and_then(|path| fs.read_to_string(Path::new(path)).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| self.claude_system_prompt.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;
    use trogon_std::fs::MemFs;

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

    #[test]
    fn dm_policy_defaults_to_open() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.dm_policy, DmPolicy::Open);
    }

    #[test]
    fn dm_policy_disabled() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env.set("SLACK_DM_POLICY", "disabled");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.dm_policy, DmPolicy::Disabled);
    }

    #[test]
    fn channel_allowlist_parsed() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env.set("SLACK_CHANNEL_ALLOWLIST", "C1, C2, C3");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.channel_allowlist, vec!["C1", "C2", "C3"]);
    }

    #[test]
    fn channel_allowlist_empty_by_default() {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.channel_allowlist.is_empty());
    }

    // ── resolve_system_prompt (MemFs) ─────────────────────────────────────────

    fn base_env() -> InMemoryEnv {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env
    }

    #[test]
    fn resolve_prompt_reads_file_via_memfs() {
        let env = base_env();
        env.set("CLAUDE_SYSTEM_PROMPT_FILE", "/prompt.txt");
        let config = SlackAgentConfig::from_env(&env);

        let fs = MemFs::new();
        fs.insert("/prompt.txt", "You are a pirate assistant.\n  ");

        assert_eq!(
            config.resolve_system_prompt(&fs).as_deref(),
            Some("You are a pirate assistant.")
        );
    }

    #[test]
    fn resolve_prompt_file_takes_precedence_over_inline() {
        let env = base_env();
        env.set("CLAUDE_SYSTEM_PROMPT_FILE", "/prompt.txt");
        env.set("CLAUDE_SYSTEM_PROMPT", "inline prompt");
        let config = SlackAgentConfig::from_env(&env);

        let fs = MemFs::new();
        fs.insert("/prompt.txt", "file prompt");

        assert_eq!(
            config.resolve_system_prompt(&fs).as_deref(),
            Some("file prompt")
        );
    }

    #[test]
    fn resolve_prompt_falls_back_to_inline_when_file_missing() {
        let env = base_env();
        env.set("CLAUDE_SYSTEM_PROMPT_FILE", "/nonexistent.txt");
        env.set("CLAUDE_SYSTEM_PROMPT", "fallback prompt");
        let config = SlackAgentConfig::from_env(&env);

        let fs = MemFs::new();

        assert_eq!(
            config.resolve_system_prompt(&fs).as_deref(),
            Some("fallback prompt")
        );
    }

    #[test]
    fn resolve_prompt_returns_none_when_neither_set() {
        let env = base_env();
        let config = SlackAgentConfig::from_env(&env);
        let fs = MemFs::new();
        assert!(config.resolve_system_prompt(&fs).is_none());
    }

    #[test]
    fn resolve_prompt_empty_file_falls_back_to_inline() {
        let env = base_env();
        env.set("CLAUDE_SYSTEM_PROMPT_FILE", "/empty.txt");
        env.set("CLAUDE_SYSTEM_PROMPT", "inline fallback");
        let config = SlackAgentConfig::from_env(&env);

        let fs = MemFs::new();
        fs.insert("/empty.txt", "   \n  ");

        assert_eq!(
            config.resolve_system_prompt(&fs).as_deref(),
            Some("inline fallback")
        );
    }

    #[test]
    fn resolve_prompt_inline_only() {
        let env = base_env();
        env.set("CLAUDE_SYSTEM_PROMPT", "You are helpful.");
        let config = SlackAgentConfig::from_env(&env);
        let fs = MemFs::new();
        assert_eq!(
            config.resolve_system_prompt(&fs).as_deref(),
            Some("You are helpful.")
        );
    }

    // ── Additional InMemoryEnv config tests ───────────────────────────────────

    #[test]
    fn welcome_message_set() {
        let env = base_env();
        env.set("SLACK_WELCOME_MESSAGE", "Welcome!");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.welcome_message.as_deref(), Some("Welcome!"));
    }

    #[test]
    fn welcome_message_not_set_is_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.welcome_message.is_none());
    }

    #[test]
    fn empty_welcome_message_treated_as_none() {
        let env = base_env();
        env.set("SLACK_WELCOME_MESSAGE", "");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.welcome_message.is_none());
    }

    #[test]
    fn claude_max_tokens_invalid_falls_back_to_default() {
        let env = base_env();
        env.set("CLAUDE_MAX_TOKENS", "not-a-number");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.claude_max_tokens, 8192);
    }

    #[test]
    fn claude_max_history_invalid_falls_back_to_default() {
        let env = base_env();
        env.set("CLAUDE_MAX_HISTORY", "abc");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.claude_max_history, 40);
    }

    #[test]
    fn health_port_invalid_falls_back_to_default() {
        let env = base_env();
        env.set("HEALTH_PORT", "not-a-port");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.health_port, 8081);
    }

    #[test]
    fn reply_to_mode_all() {
        let env = base_env();
        env.set("SLACK_REPLY_TO_MODE", "all");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.reply_to_mode, ReplyToMode::All);
    }

    #[test]
    fn anthropic_api_key_empty_string_treated_as_none() {
        let env = base_env();
        env.set("ANTHROPIC_API_KEY", "");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.anthropic_api_key.is_none());
    }

    #[test]
    fn channel_allowlist_trims_whitespace() {
        let env = base_env();
        env.set("SLACK_CHANNEL_ALLOWLIST", " C1 , C2 , C3 ");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.channel_allowlist, vec!["C1", "C2", "C3"]);
    }

    // ── New fields ────────────────────────────────────────────────────────────

    #[test]
    fn reply_to_mode_dm_defaults_to_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.reply_to_mode_dm.is_none());
    }

    #[test]
    fn reply_to_mode_dm_set() {
        let env = base_env();
        env.set("SLACK_REPLY_TO_MODE_DM", "all");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.reply_to_mode_dm, Some(ReplyToMode::All));
    }

    #[test]
    fn reply_to_mode_dm_empty_treated_as_none() {
        let env = base_env();
        env.set("SLACK_REPLY_TO_MODE_DM", "");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.reply_to_mode_dm.is_none());
    }

    #[test]
    fn reply_to_mode_group_defaults_to_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.reply_to_mode_group.is_none());
    }

    #[test]
    fn reply_to_mode_group_set() {
        let env = base_env();
        env.set("SLACK_REPLY_TO_MODE_GROUP", "first");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.reply_to_mode_group, Some(ReplyToMode::First));
    }

    #[test]
    fn reply_to_mode_group_empty_treated_as_none() {
        let env = base_env();
        env.set("SLACK_REPLY_TO_MODE_GROUP", "");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.reply_to_mode_group.is_none());
    }

    #[test]
    fn thread_initial_history_limit_defaults_to_zero() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert_eq!(config.thread_initial_history_limit, 0);
    }

    #[test]
    fn thread_initial_history_limit_set() {
        let env = base_env();
        env.set("THREAD_INITIAL_HISTORY_LIMIT", "10");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.thread_initial_history_limit, 10);
    }

    #[test]
    fn thread_initial_history_limit_invalid_falls_back_to_zero() {
        let env = base_env();
        env.set("THREAD_INITIAL_HISTORY_LIMIT", "not-a-number");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.thread_initial_history_limit, 0);
    }

    #[test]
    fn allow_bots_defaults_to_false() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(!config.allow_bots);
    }

    #[test]
    fn allow_bots_true_string() {
        let env = base_env();
        env.set("SLACK_ALLOW_BOTS", "true");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.allow_bots);
    }

    #[test]
    fn allow_bots_one_string() {
        let env = base_env();
        env.set("SLACK_ALLOW_BOTS", "1");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.allow_bots);
    }

    #[test]
    fn allow_bots_false_string() {
        let env = base_env();
        env.set("SLACK_ALLOW_BOTS", "false");
        let config = SlackAgentConfig::from_env(&env);
        assert!(!config.allow_bots);
    }

    // ── claude_max_history_chars ───────────────────────────────────────────────

    #[test]
    fn claude_max_history_chars_defaults_to_zero() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert_eq!(config.claude_max_history_chars, 0);
    }

    #[test]
    fn claude_max_history_chars_set() {
        let env = base_env();
        env.set("CLAUDE_MAX_HISTORY_CHARS", "200000");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.claude_max_history_chars, 200_000);
    }

    #[test]
    fn claude_max_history_chars_invalid_falls_back_to_zero() {
        let env = base_env();
        env.set("CLAUDE_MAX_HISTORY_CHARS", "not-a-number");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.claude_max_history_chars, 0);
    }

    #[test]
    fn claude_max_history_chars_zero_explicit() {
        let env = base_env();
        env.set("CLAUDE_MAX_HISTORY_CHARS", "0");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.claude_max_history_chars, 0);
    }
}
