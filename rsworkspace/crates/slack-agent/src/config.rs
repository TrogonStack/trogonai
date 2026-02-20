use std::collections::HashMap;
use std::path::Path;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;
use trogon_std::fs::ReadFile;
use slack_types::events::SlackSuggestedPrompt;

/// Controls who is allowed to send the bot direct messages.
#[derive(Debug, Clone, PartialEq)]
pub enum DmPolicy {
    /// Any Slack user may DM the bot (default).
    Open,
    /// All DMs are silently ignored.
    Disabled,
    /// New users must be approved via pairing code before chatting.
    Pairing,
}

impl DmPolicy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "disabled" => Self::Disabled,
            "pairing" => Self::Pairing,
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
    /// When non-empty, only messages from these user IDs are processed.
    /// Comma-separated list from SLACK_USER_ALLOWLIST.
    pub user_allowlist: Vec<String>,
    /// Messages from these user IDs are silently ignored.
    /// Comma-separated list from SLACK_USER_BLOCKLIST.
    pub user_blocklist: Vec<String>,

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

    // ── Rate limiting ─────────────────────────────────────────────────────────
    /// Max messages per user per minute. 0 = disabled (default).
    /// Configurable via SLACK_USER_RATE_LIMIT.
    pub user_rate_limit: u32,

    // ── Debouncing ────────────────────────────────────────────────────────────
    /// Debounce window in milliseconds. 0 = disabled (default).
    /// When enabled, if a user sends multiple messages within this window,
    /// only the last one is processed. Configurable via SLACK_DEBOUNCE_MS.
    pub debounce_ms: u64,

    // ── Per-channel system prompts ────────────────────────────────────────────
    /// Per-channel system prompt overrides. When a message arrives on a channel
    /// that has an entry here, this prompt replaces `base_system_prompt` for that
    /// request. Read from `CHANNEL_SYSTEM_PROMPTS` env var as:
    /// `C123=prompt text,C456=other prompt`.
    pub channel_system_prompts: HashMap<String, String>,

    // ── Reaction notifications ────────────────────────────────────────────────
    /// When true, publish acknowledgment messages on thumbsup/thumbsdown reactions.
    /// Read from SLACK_REACTION_NOTIFICATIONS env var. Default: false.
    pub reaction_notifications: bool,

    /// Number of retry attempts on transient Claude API errors. 0 = no retries.
    /// Clamped to 0..=5. Default: 3.
    pub claude_retry_attempts: u32,

    /// The bot's own Slack user ID (e.g. "U01234ABCDE"). When set and
    /// `reaction_notifications` is true, acks are only sent for reactions on
    /// the bot's own messages. Read from SLACK_BOT_USER_ID.
    pub bot_user_id: Option<String>,

    /// Maximum concurrent Claude API calls. 0 = unlimited (default).
    /// Read from SLACK_MAX_CONCURRENT_SESSIONS.
    pub max_concurrent_sessions: u32,

    /// Number of recent channel messages to fetch from Slack when a session starts
    /// with no history. 0 = disabled (default). Read from SLACK_SEED_HISTORY_ON_START.
    pub slack_seed_history_on_start: usize,

    /// Channels where the typing indicator (setStatus) is suppressed.
    /// Read from SLACK_NO_TYPING_CHANNELS as a comma-separated list of channel IDs.
    pub no_typing_channels: std::collections::HashSet<String>,

    /// When set, DM sessions share conversation history with this channel.
    /// DMs use the paired channel's session_key instead of their own.
    /// Read from SLACK_DM_PAIR_CHANNEL (a single channel ID, e.g. "C01234ABCDE").
    pub dm_pair_channel: Option<String>,

    /// When set, responses longer than this many characters are uploaded as a
    /// Markdown file instead of sent as inline text. 0 / unset = disabled.
    /// Read from SLACK_UPLOAD_THRESHOLD_CHARS.
    pub upload_threshold_chars: Option<usize>,

    /// Suggested prompts to show in the Slack assistant thread UI.
    /// Parsed from SLACK_SUGGESTED_PROMPTS as "Title1:Message1,Title2:Message2".
    /// Empty or missing = no suggested prompts.
    pub suggested_prompts: Vec<SlackSuggestedPrompt>,

    // ── History scope ─────────────────────────────────────────────────────────
    /// History scope for thread messages. "thread" uses thread history (default),
    /// "parent" uses the parent channel history. Read from SLACK_HISTORY_SCOPE.
    pub history_scope_parent: bool,

    // ── Inherit parent session ────────────────────────────────────────────────
    /// When true, new thread sessions are seeded with the parent channel's history.
    /// Read from SLACK_INHERIT_PARENT (default: false).
    pub inherit_parent: bool,

    // ── Per-chat-type reply mode for channel ──────────────────────────────────
    /// Reply threading mode override for channel messages (not DM or group).
    /// Read from SLACK_REPLY_TO_MODE_CHANNEL.
    pub reply_to_mode_channel: Option<ReplyToMode>,

    // ── Per-channel user allowlists ───────────────────────────────────────────
    /// Per-channel user allowlists. Format: CHANNEL_USER_ALLOWLISTS=C123:U1,U2;C456:U3
    /// If a channel has an entry, only those users can interact in that channel.
    pub channel_user_allowlists: HashMap<String, Vec<String>>,

    // ── Action feature flags ──────────────────────────────────────────────────
    /// Feature flags to enable/disable action groups.
    pub actions_reactions: bool,   // SLACK_ACTIONS_REACTIONS (default: true)
    pub actions_pins: bool,        // SLACK_ACTIONS_PINS (default: true)
    pub actions_member_info: bool, // SLACK_ACTIONS_MEMBER_INFO (default: true)
    pub actions_emoji_list: bool,  // SLACK_ACTIONS_EMOJI_LIST (default: true)

    // ── Slash command single-mode ─────────────────────────────────────────────
    /// If set, only process slash commands with this exact name (without leading /).
    /// E.g., "openclaw" to only respond to /openclaw. Empty/unset = respond to all.
    /// Read from SLACK_SLASH_COMMAND_NAME.
    pub slash_command_name: Option<String>,

    /// If true (default), slash command responses use response_type "ephemeral"
    /// (only visible to the invoking user). If false, responses are "in_channel".
    /// Read from SLACK_SLASH_COMMAND_EPHEMERAL (default: true).
    pub slash_command_ephemeral: bool,
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

        let user_allowlist = env
            .var("SLACK_USER_ALLOWLIST")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();

        let user_blocklist = env
            .var("SLACK_USER_BLOCKLIST")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
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

        let user_rate_limit = env
            .var("SLACK_USER_RATE_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let debounce_ms = env
            .var("SLACK_DEBOUNCE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let channel_system_prompts = env
            .var("CHANNEL_SYSTEM_PROMPTS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.split(',')
                    .filter_map(|pair| {
                        let eq_pos = pair.find('=')?;
                        let channel = pair[..eq_pos].trim();
                        let prompt = pair[eq_pos + 1..].trim();
                        if channel.is_empty() || prompt.is_empty() {
                            return None;
                        }
                        Some((channel.to_string(), prompt.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let reaction_notifications = env
            .var("SLACK_REACTION_NOTIFICATIONS")
            .ok()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let claude_retry_attempts = env
            .var("CLAUDE_RETRY_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(3)
            .min(5);

        let bot_user_id = env.var("SLACK_BOT_USER_ID").ok().filter(|v| !v.is_empty());

        let max_concurrent_sessions = env
            .var("SLACK_MAX_CONCURRENT_SESSIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let slack_seed_history_on_start = env
            .var("SLACK_SEED_HISTORY_ON_START")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let no_typing_channels = env
            .var("SLACK_NO_TYPING_CHANNELS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let dm_pair_channel = env
            .var("SLACK_DM_PAIR_CHANNEL")
            .ok()
            .filter(|v| !v.is_empty());

        let upload_threshold_chars = env
            .var("SLACK_UPLOAD_THRESHOLD_CHARS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0);

        let suggested_prompts = env
            .var("SLACK_SUGGESTED_PROMPTS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.split(',')
                    .filter_map(|entry| {
                        let colon = entry.find(':')?;
                        let title = entry[..colon].trim().to_string();
                        let message = entry[colon + 1..].trim().to_string();
                        if title.is_empty() || message.is_empty() {
                            return None;
                        }
                        Some(SlackSuggestedPrompt { title, message })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let history_scope_parent = env
            .var("SLACK_HISTORY_SCOPE")
            .ok()
            .map(|v| v == "parent")
            .unwrap_or(false);

        let inherit_parent = env
            .var("SLACK_INHERIT_PARENT")
            .ok()
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let reply_to_mode_channel = env
            .var("SLACK_REPLY_TO_MODE_CHANNEL")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| ReplyToMode::from_str(&v));

        let channel_user_allowlists = env
            .var("CHANNEL_USER_ALLOWLISTS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.split(';')
                    .filter_map(|entry| {
                        let colon = entry.find(':')?;
                        let channel_id = entry[..colon].trim().to_string();
                        let users_str = &entry[colon + 1..];
                        let users: Vec<String> = users_str
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        if channel_id.is_empty() || users.is_empty() {
                            return None;
                        }
                        Some((channel_id, users))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let actions_reactions = env
            .var("SLACK_ACTIONS_REACTIONS")
            .ok()
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let actions_pins = env
            .var("SLACK_ACTIONS_PINS")
            .ok()
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let actions_member_info = env
            .var("SLACK_ACTIONS_MEMBER_INFO")
            .ok()
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let actions_emoji_list = env
            .var("SLACK_ACTIONS_EMOJI_LIST")
            .ok()
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let slash_command_name = env
            .var("SLACK_SLASH_COMMAND_NAME")
            .ok()
            .filter(|v| !v.is_empty());

        let slash_command_ephemeral = env
            .var("SLACK_SLASH_COMMAND_EPHEMERAL")
            .ok()
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

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
            user_allowlist,
            user_blocklist,
            reply_to_mode_dm,
            reply_to_mode_group,
            thread_initial_history_limit,
            allow_bots,
            user_rate_limit,
            debounce_ms,
            channel_system_prompts,
            reaction_notifications,
            claude_retry_attempts,
            bot_user_id,
            max_concurrent_sessions,
            slack_seed_history_on_start,
            no_typing_channels,
            dm_pair_channel,
            upload_threshold_chars,
            suggested_prompts,
            history_scope_parent,
            inherit_parent,
            reply_to_mode_channel,
            channel_user_allowlists,
            actions_reactions,
            actions_pins,
            actions_member_info,
            actions_emoji_list,
            slash_command_name,
            slash_command_ephemeral,
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

    // ── user_rate_limit ────────────────────────────────────────────────────────

    #[test]
    fn user_rate_limit_defaults_to_zero() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert_eq!(config.user_rate_limit, 0);
    }

    #[test]
    fn user_rate_limit_set_to_ten() {
        let env = base_env();
        env.set("SLACK_USER_RATE_LIMIT", "10");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.user_rate_limit, 10);
    }

    #[test]
    fn user_rate_limit_invalid_falls_back_to_zero() {
        let env = base_env();
        env.set("SLACK_USER_RATE_LIMIT", "not-a-number");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.user_rate_limit, 0);
    }

    // ── debounce_ms ────────────────────────────────────────────────────────────

    #[test]
    fn debounce_ms_defaults_to_zero() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert_eq!(config.debounce_ms, 0);
    }

    #[test]
    fn debounce_ms_set() {
        let env = base_env();
        env.set("SLACK_DEBOUNCE_MS", "500");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.debounce_ms, 500);
    }

    #[test]
    fn debounce_ms_invalid_falls_back_to_zero() {
        let env = base_env();
        env.set("SLACK_DEBOUNCE_MS", "not-a-number");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.debounce_ms, 0);
    }

    // ── channel_system_prompts ────────────────────────────────────────────────

    #[test]
    fn channel_system_prompts_defaults_to_empty() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.channel_system_prompts.is_empty());
    }

    #[test]
    fn channel_system_prompts_parsed() {
        let env = base_env();
        env.set("CHANNEL_SYSTEM_PROMPTS", "C123=You are a helper,C456=You are a coder");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.channel_system_prompts.get("C123").map(|s| s.as_str()), Some("You are a helper"));
        assert_eq!(config.channel_system_prompts.get("C456").map(|s| s.as_str()), Some("You are a coder"));
        assert_eq!(config.channel_system_prompts.len(), 2);
    }

    #[test]
    fn channel_system_prompts_trims_whitespace() {
        let env = base_env();
        env.set("CHANNEL_SYSTEM_PROMPTS", " C123 = You are a helper , C456 = coder ");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.channel_system_prompts.get("C123").map(|s| s.as_str()), Some("You are a helper"));
        assert_eq!(config.channel_system_prompts.get("C456").map(|s| s.as_str()), Some("coder"));
    }

    #[test]
    fn channel_system_prompts_prompt_with_equals_sign() {
        // Prompt text can contain '=' — only the FIRST '=' is the delimiter
        let env = base_env();
        env.set("CHANNEL_SYSTEM_PROMPTS", "C123=a=b=c");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.channel_system_prompts.get("C123").map(|s| s.as_str()), Some("a=b=c"));
    }

    // ── reaction_notifications ────────────────────────────────────────────────

    #[test]
    fn reaction_notifications_defaults_to_false() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(!config.reaction_notifications);
    }

    #[test]
    fn reaction_notifications_enabled() {
        let env = base_env();
        env.set("SLACK_REACTION_NOTIFICATIONS", "true");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.reaction_notifications);

        let env2 = base_env();
        env2.set("SLACK_REACTION_NOTIFICATIONS", "1");
        let config2 = SlackAgentConfig::from_env(&env2);
        assert!(config2.reaction_notifications);
    }

    // ── user_allowlist ────────────────────────────────────────────────────────

    #[test]
    fn user_allowlist_defaults_to_empty() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.user_allowlist.is_empty());
    }

    #[test]
    fn user_allowlist_parsed() {
        let env = base_env();
        env.set("SLACK_USER_ALLOWLIST", "U1, U2, U3");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.user_allowlist, vec!["U1", "U2", "U3"]);
    }

    // ── user_blocklist ────────────────────────────────────────────────────────

    #[test]
    fn user_blocklist_defaults_to_empty() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.user_blocklist.is_empty());
    }

    #[test]
    fn user_blocklist_parsed() {
        let env = base_env();
        env.set("SLACK_USER_BLOCKLIST", "U4, U5");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.user_blocklist, vec!["U4", "U5"]);
    }

    // ── claude_retry_attempts ─────────────────────────────────────────────────

    #[test]
    fn claude_retry_attempts_defaults_to_three() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert_eq!(config.claude_retry_attempts, 3);
    }

    #[test]
    fn claude_retry_attempts_set() {
        let env = base_env();
        env.set("CLAUDE_RETRY_ATTEMPTS", "2");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.claude_retry_attempts, 2);
    }

    #[test]
    fn claude_retry_attempts_clamped_to_five() {
        let env = base_env();
        env.set("CLAUDE_RETRY_ATTEMPTS", "10");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.claude_retry_attempts, 5);
    }

    // ── bot_user_id ───────────────────────────────────────────────────────────

    #[test]
    fn bot_user_id_defaults_to_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.bot_user_id.is_none());
    }

    #[test]
    fn bot_user_id_set() {
        let env = base_env();
        env.set("SLACK_BOT_USER_ID", "U01234ABCDE");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.bot_user_id.as_deref(), Some("U01234ABCDE"));
    }

    // ── max_concurrent_sessions ───────────────────────────────────────────────

    #[test]
    fn max_concurrent_sessions_defaults_to_zero() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert_eq!(config.max_concurrent_sessions, 0);
    }

    #[test]
    fn max_concurrent_sessions_set() {
        let env = base_env();
        env.set("SLACK_MAX_CONCURRENT_SESSIONS", "5");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.max_concurrent_sessions, 5);
    }

    // ── slack_seed_history_on_start ───────────────────────────────────────────

    #[test]
    fn slack_seed_history_on_start_defaults_to_zero() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert_eq!(config.slack_seed_history_on_start, 0);
    }

    #[test]
    fn slack_seed_history_on_start_set() {
        let env = base_env();
        env.set("SLACK_SEED_HISTORY_ON_START", "20");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.slack_seed_history_on_start, 20);
    }

    // ── no_typing_channels ────────────────────────────────────────────────────

    #[test]
    fn no_typing_channels_defaults_to_empty() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.no_typing_channels.is_empty());
    }

    #[test]
    fn no_typing_channels_parsed() {
        let env = base_env();
        env.set("SLACK_NO_TYPING_CHANNELS", "C111, C222, C333");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.no_typing_channels.contains("C111"));
        assert!(config.no_typing_channels.contains("C222"));
        assert!(config.no_typing_channels.contains("C333"));
        assert_eq!(config.no_typing_channels.len(), 3);
    }

    // ── dm_pair_channel ───────────────────────────────────────────────────────

    #[test]
    fn dm_pair_channel_defaults_to_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.dm_pair_channel.is_none());
    }

    #[test]
    fn dm_pair_channel_set() {
        let env = base_env();
        env.set("SLACK_DM_PAIR_CHANNEL", "C01234ABCDE");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.dm_pair_channel.as_deref(), Some("C01234ABCDE"));
    }

    // ── upload_threshold_chars ────────────────────────────────────────────────

    #[test]
    fn upload_threshold_defaults_to_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.upload_threshold_chars.is_none());
    }

    #[test]
    fn upload_threshold_zero_is_disabled() {
        let env = base_env();
        env.set("SLACK_UPLOAD_THRESHOLD_CHARS", "0");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.upload_threshold_chars.is_none());
    }

    #[test]
    fn upload_threshold_set() {
        let env = base_env();
        env.set("SLACK_UPLOAD_THRESHOLD_CHARS", "3000");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.upload_threshold_chars, Some(3000));
    }

    // ── suggested_prompts ─────────────────────────────────────────────────────

    #[test]
    fn suggested_prompts_defaults_to_empty() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.suggested_prompts.is_empty());
    }

    #[test]
    fn suggested_prompts_single_entry() {
        let env = base_env();
        env.set("SLACK_SUGGESTED_PROMPTS", "Hello:Say hello");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.suggested_prompts.len(), 1);
        assert_eq!(config.suggested_prompts[0].title, "Hello");
        assert_eq!(config.suggested_prompts[0].message, "Say hello");
    }

    #[test]
    fn suggested_prompts_multiple_entries() {
        let env = base_env();
        env.set("SLACK_SUGGESTED_PROMPTS", "Greet:Say hello,Help:What can you do?");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.suggested_prompts.len(), 2);
        assert_eq!(config.suggested_prompts[0].title, "Greet");
        assert_eq!(config.suggested_prompts[0].message, "Say hello");
        assert_eq!(config.suggested_prompts[1].title, "Help");
        assert_eq!(config.suggested_prompts[1].message, "What can you do?");
    }

    // ── dm_policy pairing ─────────────────────────────────────────────────────

    #[test]
    fn dm_policy_pairing() {
        let env = base_env();
        env.set("SLACK_DM_POLICY", "pairing");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.dm_policy, DmPolicy::Pairing);
    }

    // ── history_scope_parent ──────────────────────────────────────────────────

    #[test]
    fn history_scope_parent_defaults_to_false() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(!config.history_scope_parent);
    }

    #[test]
    fn history_scope_parent_set_to_parent() {
        let env = base_env();
        env.set("SLACK_HISTORY_SCOPE", "parent");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.history_scope_parent);
    }

    #[test]
    fn history_scope_thread_is_false() {
        let env = base_env();
        env.set("SLACK_HISTORY_SCOPE", "thread");
        let config = SlackAgentConfig::from_env(&env);
        assert!(!config.history_scope_parent);
    }

    // ── inherit_parent ────────────────────────────────────────────────────────

    #[test]
    fn inherit_parent_defaults_to_false() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(!config.inherit_parent);
    }

    #[test]
    fn inherit_parent_set_true() {
        let env = base_env();
        env.set("SLACK_INHERIT_PARENT", "true");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.inherit_parent);
    }

    #[test]
    fn inherit_parent_set_one() {
        let env = base_env();
        env.set("SLACK_INHERIT_PARENT", "1");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.inherit_parent);
    }

    #[test]
    fn inherit_parent_set_false() {
        let env = base_env();
        env.set("SLACK_INHERIT_PARENT", "false");
        let config = SlackAgentConfig::from_env(&env);
        assert!(!config.inherit_parent);
    }

    // ── reply_to_mode_channel ─────────────────────────────────────────────────

    #[test]
    fn reply_to_mode_channel_defaults_to_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.reply_to_mode_channel.is_none());
    }

    #[test]
    fn reply_to_mode_channel_set() {
        let env = base_env();
        env.set("SLACK_REPLY_TO_MODE_CHANNEL", "all");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.reply_to_mode_channel, Some(ReplyToMode::All));
    }

    #[test]
    fn reply_to_mode_channel_empty_treated_as_none() {
        let env = base_env();
        env.set("SLACK_REPLY_TO_MODE_CHANNEL", "");
        let config = SlackAgentConfig::from_env(&env);
        assert!(config.reply_to_mode_channel.is_none());
    }

    // ── channel_user_allowlists ───────────────────────────────────────────────

    #[test]
    fn channel_user_allowlists_defaults_to_empty() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.channel_user_allowlists.is_empty());
    }

    #[test]
    fn channel_user_allowlists_parsed() {
        let env = base_env();
        env.set("CHANNEL_USER_ALLOWLISTS", "C123:U1,U2;C456:U3");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.channel_user_allowlists.get("C123"), Some(&vec!["U1".to_string(), "U2".to_string()]));
        assert_eq!(config.channel_user_allowlists.get("C456"), Some(&vec!["U3".to_string()]));
    }

    #[test]
    fn channel_user_allowlists_trims_whitespace() {
        let env = base_env();
        env.set("CHANNEL_USER_ALLOWLISTS", " C123 : U1 , U2 ; C456 : U3 ");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.channel_user_allowlists.get("C123"), Some(&vec!["U1".to_string(), "U2".to_string()]));
        assert_eq!(config.channel_user_allowlists.get("C456"), Some(&vec!["U3".to_string()]));
    }

    // ── actions_* feature flags ───────────────────────────────────────────────

    #[test]
    fn actions_reactions_defaults_to_true() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.actions_reactions);
    }

    #[test]
    fn actions_reactions_disabled() {
        let env = base_env();
        env.set("SLACK_ACTIONS_REACTIONS", "false");
        let config = SlackAgentConfig::from_env(&env);
        assert!(!config.actions_reactions);
    }

    #[test]
    fn actions_pins_defaults_to_true() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.actions_pins);
    }

    #[test]
    fn actions_pins_disabled() {
        let env = base_env();
        env.set("SLACK_ACTIONS_PINS", "0");
        let config = SlackAgentConfig::from_env(&env);
        assert!(!config.actions_pins);
    }

    #[test]
    fn actions_member_info_defaults_to_true() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.actions_member_info);
    }

    #[test]
    fn actions_emoji_list_defaults_to_true() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.actions_emoji_list);
    }
    // ── slash_command_name ────────────────────────────────────────────────────

    #[test]
    fn slash_command_name_defaults_to_none() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.slash_command_name.is_none());
    }

    #[test]
    fn slash_command_name_set() {
        let env = base_env();
        env.set("SLACK_SLASH_COMMAND_NAME", "openclaw");
        let config = SlackAgentConfig::from_env(&env);
        assert_eq!(config.slash_command_name, Some("openclaw".to_string()));
    }

    // ── slash_command_ephemeral ───────────────────────────────────────────────

    #[test]
    fn slash_command_ephemeral_defaults_to_true() {
        let config = SlackAgentConfig::from_env(&base_env());
        assert!(config.slash_command_ephemeral);
    }

    #[test]
    fn slash_command_ephemeral_set_false() {
        let env = base_env();
        env.set("SLACK_SLASH_COMMAND_EPHEMERAL", "false");
        let config = SlackAgentConfig::from_env(&env);
        assert!(!config.slash_command_ephemeral);
    }

}