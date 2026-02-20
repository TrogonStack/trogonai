use std::collections::HashSet;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

/// How the bot receives events from Slack.
#[derive(Debug, Clone, PartialEq)]
pub enum BotMode {
    /// Socket Mode (default): bot opens a WebSocket connection to Slack.
    /// Requires SLACK_APP_TOKEN (xapp-...).
    Socket,
    /// HTTP Events API: Slack POSTs events to our HTTP endpoint.
    /// Does not require SLACK_APP_TOKEN; uses SLACK_SIGNING_SECRET for verification.
    Http,
}

impl BotMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "http" => Self::Http,
            _ => Self::Socket,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlackBotConfig {
    /// Bot token (xoxb-...) — used to post messages to Slack.
    pub bot_token: String,
    /// App-level token (xapp-...) — used to open the Socket Mode connection.
    pub app_token: Option<String>,
    /// Optional bot user ID (UXXXXXXXX). When set, messages from this user
    /// are filtered in the listener as an additional guard against loops.
    pub bot_user_id: Option<String>,
    /// When true (default), channel/group messages only reach the agent if the
    /// bot is @mentioned or if the message is a thread reply. DMs always pass.
    /// Mirrors OpenClaw's `requireMention` default behaviour.
    pub mention_gating: bool,
    /// Channels where mention gating is always ON, regardless of `mention_gating`.
    /// Comma-separated channel IDs in `SLACK_MENTION_GATING_CHANNELS`.
    pub mention_gating_channels: HashSet<String>,
    /// Channels where mention gating is always OFF, regardless of `mention_gating`.
    /// Comma-separated channel IDs in `SLACK_NO_MENTION_CHANNELS`.
    pub no_mention_channels: HashSet<String>,
    /// Custom text patterns (beyond @mention) that activate the bot even when
    /// mention gating is enabled. Comma-separated substrings in `SLACK_MENTION_PATTERNS`.
    pub mention_patterns: Vec<String>,
    /// When true, messages from bots are forwarded to NATS instead of being
    /// silently dropped. Default: false.
    pub allow_bots: bool,
    pub nats: NatsConfig,
    /// Port for the HTTP health check endpoint. Default: 8080.
    pub health_port: u16,
    /// Slack signing secret for verifying webhook requests.
    /// When set, all webhook requests are signature-verified.
    pub signing_secret: Option<String>,
    /// Port for the raw HTTP Events API webhook server (for pin events).
    /// Default: 3001.
    pub events_port: u16,
    /// Max outbound Slack API requests per second. Default: 1.0.
    /// Configurable via SLACK_API_RPS. Clamped to [0.1, 50.0].
    pub slack_api_rps: f32,
    /// Maximum file size in MB for inbound media downloads. Default: 20.
    /// Read from SLACK_MEDIA_MAX_MB.
    pub media_max_mb: u64,
    /// Maximum characters per Slack message chunk. Default: 4000.
    /// Read from SLACK_TEXT_CHUNK_LIMIT.
    pub text_chunk_limit: usize,
    /// Text chunking mode: "chars" (default) splits at character limit,
    /// "newline" prefers splitting on paragraph/line boundaries first.
    /// Read from SLACK_CHUNK_MODE.
    pub chunk_mode_newline: bool,
    /// Optional Slack user token (xoxp-...) for read-only API calls that
    /// require user-level permissions. Read from SLACK_USER_TOKEN.
    pub user_token: Option<String>,
    /// Connection mode. Read from SLACK_MODE ("socket" or "http"). Default: socket.
    pub mode: BotMode,
    /// HTTP path for Events API webhook. Read from SLACK_HTTP_PATH. Default: "/slack/events".
    pub http_path: String,
}

impl SlackBotConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let bot_token = env
            .var("SLACK_BOT_TOKEN")
            .expect("SLACK_BOT_TOKEN must be set");
        let app_token = env.var("SLACK_APP_TOKEN").ok();
        let bot_user_id = env.var("SLACK_BOT_USER_ID").ok();
        // Mention-gating mirrors OpenClaw's `requireMention` default: true.
        // Set SLACK_MENTION_GATING=false to respond to all channel messages.
        let mention_gating = env
            .var("SLACK_MENTION_GATING")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);
        // Per-channel overrides: comma-separated channel ID lists.
        let mention_gating_channels = env
            .var("SLACK_MENTION_GATING_CHANNELS")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let no_mention_channels = env
            .var("SLACK_NO_MENTION_CHANNELS")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let mention_patterns = env
            .var("SLACK_MENTION_PATTERNS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let allow_bots = env
            .var("SLACK_ALLOW_BOTS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let nats = NatsConfig::from_env(env);
        let health_port = env
            .var("HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);
        let signing_secret = env.var("SLACK_SIGNING_SECRET").ok();
        let events_port = env
            .var("SLACK_EVENTS_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3001);
        let slack_api_rps = env
            .var("SLACK_API_RPS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0)
            .clamp(0.1, 50.0);
        let media_max_mb = env
            .var("SLACK_MEDIA_MAX_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(20);
        let text_chunk_limit = env
            .var("SLACK_TEXT_CHUNK_LIMIT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4000);
        let chunk_mode_newline = env
            .var("SLACK_CHUNK_MODE")
            .map(|v| v == "newline")
            .unwrap_or(false);
        let user_token = env
            .var("SLACK_USER_TOKEN")
            .ok()
            .filter(|v| !v.is_empty());
        let mode = env
            .var("SLACK_MODE")
            .map(|v| BotMode::from_str(&v))
            .unwrap_or(BotMode::Socket);
        let http_path = env
            .var("SLACK_HTTP_PATH")
            .unwrap_or_else(|_| "/slack/events".to_string());

        Self {
            bot_token,
            app_token,
            bot_user_id,
            mention_gating,
            mention_gating_channels,
            no_mention_channels,
            mention_patterns,
            allow_bots,
            nats,
            health_port,
            signing_secret,
            events_port,
            slack_api_rps,
            media_max_mb,
            text_chunk_limit,
            chunk_mode_newline,
            user_token,
            mode,
            http_path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_std::env::InMemoryEnv;

    fn base_env() -> InMemoryEnv {
        let env = InMemoryEnv::new();
        env.set("NATS_URL", "nats://localhost:4222");
        env.set("SLACK_BOT_TOKEN", "xoxb-test");
        env
    }

    #[test]
    fn from_env_defaults() {
        let config = SlackBotConfig::from_env(&base_env());
        assert_eq!(config.bot_token, "xoxb-test");
        assert!(config.app_token.is_none());
        assert!(config.bot_user_id.is_none());
        assert!(config.mention_gating);
        assert_eq!(config.health_port, 8080);
    }

    #[test]
    fn from_env_custom_values() {
        let env = base_env();
        env.set("SLACK_BOT_USER_ID", "UBOT001");
        env.set("SLACK_MENTION_GATING", "false");
        env.set("HEALTH_PORT", "9090");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.bot_user_id.as_deref(), Some("UBOT001"));
        assert!(!config.mention_gating);
        assert_eq!(config.health_port, 9090);
    }

    #[test]
    fn mention_gating_disabled_via_zero() {
        let env = base_env();
        env.set("SLACK_MENTION_GATING", "0");
        assert!(!SlackBotConfig::from_env(&env).mention_gating);
    }

    #[test]
    fn mention_gating_enabled_when_set_to_true() {
        let env = base_env();
        env.set("SLACK_MENTION_GATING", "true");
        assert!(SlackBotConfig::from_env(&env).mention_gating);
    }

    #[test]
    fn mention_gating_default_is_true() {
        assert!(SlackBotConfig::from_env(&base_env()).mention_gating);
    }

    #[test]
    fn mention_gating_any_other_value_is_true() {
        let env = base_env();
        env.set("SLACK_MENTION_GATING", "yes");
        assert!(SlackBotConfig::from_env(&env).mention_gating);
    }

    #[test]
    fn bot_user_id_set() {
        let env = base_env();
        env.set("SLACK_BOT_USER_ID", "UBOT123");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.bot_user_id.as_deref(), Some("UBOT123"));
    }

    #[test]
    fn bot_user_id_not_set_is_none() {
        assert!(SlackBotConfig::from_env(&base_env()).bot_user_id.is_none());
    }

    #[test]
    fn health_port_custom() {
        let env = base_env();
        env.set("HEALTH_PORT", "9999");
        assert_eq!(SlackBotConfig::from_env(&env).health_port, 9999);
    }

    #[test]
    fn health_port_invalid_falls_back_to_default() {
        let env = base_env();
        env.set("HEALTH_PORT", "bad");
        assert_eq!(SlackBotConfig::from_env(&env).health_port, 8080);
    }

    #[test]
    fn allow_bots_default_is_false() {
        assert!(!SlackBotConfig::from_env(&base_env()).allow_bots);
    }

    #[test]
    fn allow_bots_enabled() {
        let env = base_env();
        env.set("SLACK_ALLOW_BOTS", "true");
        assert!(SlackBotConfig::from_env(&env).allow_bots);

        let env2 = base_env();
        env2.set("SLACK_ALLOW_BOTS", "1");
        assert!(SlackBotConfig::from_env(&env2).allow_bots);
    }

    #[test]
    fn signing_secret_not_set_is_none() {
        assert!(SlackBotConfig::from_env(&base_env()).signing_secret.is_none());
    }

    #[test]
    fn signing_secret_set() {
        let env = base_env();
        env.set("SLACK_SIGNING_SECRET", "secret123");
        assert_eq!(SlackBotConfig::from_env(&env).signing_secret.as_deref(), Some("secret123"));
    }

    #[test]
    fn events_port_default() {
        assert_eq!(SlackBotConfig::from_env(&base_env()).events_port, 3001);
    }

    #[test]
    fn events_port_custom() {
        let env = base_env();
        env.set("SLACK_EVENTS_PORT", "4001");
        assert_eq!(SlackBotConfig::from_env(&env).events_port, 4001);
    }

    #[test]
    fn mention_gating_channels_default_empty() {
        let config = SlackBotConfig::from_env(&base_env());
        assert!(config.mention_gating_channels.is_empty());
        assert!(config.no_mention_channels.is_empty());
    }

    #[test]
    fn mention_gating_channels_parsed() {
        let env = base_env();
        env.set("SLACK_MENTION_GATING_CHANNELS", "C111,C222, C333 ");
        let config = SlackBotConfig::from_env(&env);
        assert!(config.mention_gating_channels.contains("C111"));
        assert!(config.mention_gating_channels.contains("C222"));
        assert!(config.mention_gating_channels.contains("C333"));
        assert_eq!(config.mention_gating_channels.len(), 3);
    }

    #[test]
    fn no_mention_channels_parsed() {
        let env = base_env();
        env.set("SLACK_NO_MENTION_CHANNELS", "C444,C555");
        let config = SlackBotConfig::from_env(&env);
        assert!(config.no_mention_channels.contains("C444"));
        assert!(config.no_mention_channels.contains("C555"));
        assert_eq!(config.no_mention_channels.len(), 2);
    }

    #[test]
    fn slack_api_rps_default_is_1() {
        let config = SlackBotConfig::from_env(&base_env());
        assert!((config.slack_api_rps - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn slack_api_rps_custom_value() {
        let env = base_env();
        env.set("SLACK_API_RPS", "5.0");
        let config = SlackBotConfig::from_env(&env);
        assert!((config.slack_api_rps - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn slack_api_rps_invalid_falls_back_to_default() {
        let env = base_env();
        env.set("SLACK_API_RPS", "not_a_number");
        let config = SlackBotConfig::from_env(&env);
        assert!((config.slack_api_rps - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn slack_api_rps_clamped_to_max() {
        let env = base_env();
        env.set("SLACK_API_RPS", "9999.0");
        let config = SlackBotConfig::from_env(&env);
        assert!((config.slack_api_rps - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn slack_api_rps_clamped_to_min() {
        let env = base_env();
        env.set("SLACK_API_RPS", "0.0");
        let config = SlackBotConfig::from_env(&env);
        assert!((config.slack_api_rps - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn mention_patterns_defaults_to_empty() {
        let env = base_env();
        let config = SlackBotConfig::from_env(&env);
        assert!(config.mention_patterns.is_empty());
    }

    #[test]
    fn mention_patterns_parsed() {
        let env = base_env();
        env.set("SLACK_MENTION_PATTERNS", "hey bot, help me, assistant");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.mention_patterns, vec!["hey bot", "help me", "assistant"]);
    }

    #[test]
    fn mention_patterns_trims_whitespace() {
        let env = base_env();
        env.set("SLACK_MENTION_PATTERNS", " hello , world ");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.mention_patterns, vec!["hello", "world"]);
    }

    #[test]
    fn media_max_mb_default_is_20() {
        let config = SlackBotConfig::from_env(&base_env());
        assert_eq!(config.media_max_mb, 20);
    }

    #[test]
    fn media_max_mb_custom_value() {
        let env = base_env();
        env.set("SLACK_MEDIA_MAX_MB", "50");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.media_max_mb, 50);
    }

    #[test]
    fn media_max_mb_invalid_falls_back_to_default() {
        let env = base_env();
        env.set("SLACK_MEDIA_MAX_MB", "not_a_number");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.media_max_mb, 20);
    }

    // --- Feature 1: text_chunk_limit ---

    #[test]
    fn text_chunk_limit_default_is_4000() {
        let config = SlackBotConfig::from_env(&base_env());
        assert_eq!(config.text_chunk_limit, 4000);
    }

    #[test]
    fn text_chunk_limit_custom_value() {
        let env = base_env();
        env.set("SLACK_TEXT_CHUNK_LIMIT", "2000");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.text_chunk_limit, 2000);
    }

    // --- Feature 2: chunk_mode_newline ---

    #[test]
    fn chunk_mode_newline_default_is_false() {
        let config = SlackBotConfig::from_env(&base_env());
        assert!(!config.chunk_mode_newline);
    }

    #[test]
    fn chunk_mode_newline_set_to_newline() {
        let env = base_env();
        env.set("SLACK_CHUNK_MODE", "newline");
        let config = SlackBotConfig::from_env(&env);
        assert!(config.chunk_mode_newline);
    }

    // --- Feature 3: user_token ---

    #[test]
    fn user_token_default_is_none() {
        let config = SlackBotConfig::from_env(&base_env());
        assert!(config.user_token.is_none());
    }

    #[test]
    fn user_token_set() {
        let env = base_env();
        env.set("SLACK_USER_TOKEN", "xoxp-test-token");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.user_token.as_deref(), Some("xoxp-test-token"));
    }

    #[test]
    fn user_token_empty_string_is_none() {
        let env = base_env();
        env.set("SLACK_USER_TOKEN", "");
        let config = SlackBotConfig::from_env(&env);
        assert!(config.user_token.is_none());
    }

    #[test]
    fn mode_defaults_to_socket() {
        let config = SlackBotConfig::from_env(&base_env());
        assert_eq!(config.mode, BotMode::Socket);
    }

    #[test]
    fn mode_set_to_http() {
        let env = base_env();
        env.set("SLACK_MODE", "http");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.mode, BotMode::Http);
    }

    #[test]
    fn mode_unknown_falls_back_to_socket() {
        let env = base_env();
        env.set("SLACK_MODE", "websocket");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.mode, BotMode::Socket);
    }

    #[test]
    fn http_path_default() {
        let config = SlackBotConfig::from_env(&base_env());
        assert_eq!(config.http_path, "/slack/events");
    }

    #[test]
    fn http_path_custom() {
        let env = base_env();
        env.set("SLACK_HTTP_PATH", "/my/slack/hook");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.http_path, "/my/slack/hook");
    }

    #[test]
    fn app_token_is_optional() {
        // Should not panic even when SLACK_APP_TOKEN is not set.
        let env = base_env();
        let config = SlackBotConfig::from_env(&env);
        assert!(config.app_token.is_none());
    }

    #[test]
    fn app_token_set() {
        let env = base_env();
        env.set("SLACK_APP_TOKEN", "xapp-test-token");
        let config = SlackBotConfig::from_env(&env);
        assert_eq!(config.app_token.as_deref(), Some("xapp-test-token"));
    }
}
