use std::collections::HashSet;
use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

#[derive(Debug, Clone)]
pub struct EnricherConfig {
    pub bot_token: String,
    pub bot_user_id: Option<String>,
    pub mention_gating: bool,
    pub mention_gating_channels: HashSet<String>,
    pub no_mention_channels: HashSet<String>,
    pub mention_patterns: Vec<String>,
    pub allow_bots: bool,
    pub media_max_mb: u64,
    pub account_id: Option<String>,
    pub nats: NatsConfig,
    pub health_port: u16,
}

impl EnricherConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let bot_token = env
            .var("SLACK_BOT_TOKEN")
            .expect("SLACK_BOT_TOKEN must be set");
        let bot_user_id = env.var("SLACK_BOT_USER_ID").ok();
        let mention_gating = env
            .var("SLACK_MENTION_GATING")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);
        let mention_gating_channels = env
            .var("SLACK_MENTION_GATING_CHANNELS")
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let no_mention_channels = env
            .var("SLACK_NO_MENTION_CHANNELS")
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
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
        let media_max_mb = env
            .var("SLACK_MEDIA_MAX_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(20);
        let account_id = env
            .var("SLACK_ACCOUNT_ID")
            .ok()
            .filter(|v| !v.is_empty());
        let nats = NatsConfig::from_env(env);
        let health_port = env
            .var("HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8082);

        Self {
            bot_token,
            bot_user_id,
            mention_gating,
            mention_gating_channels,
            no_mention_channels,
            mention_patterns,
            allow_bots,
            media_max_mb,
            account_id,
            nats,
            health_port,
        }
    }
}
