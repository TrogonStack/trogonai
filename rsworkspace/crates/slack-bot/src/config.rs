use trogon_nats::NatsConfig;
use trogon_std::env::ReadEnv;

#[derive(Debug, Clone)]
pub struct SlackBotConfig {
    /// Bot token (xoxb-...) — used to post messages to Slack.
    pub bot_token: String,
    /// App-level token (xapp-...) — used to open the Socket Mode connection.
    pub app_token: String,
    /// Optional bot user ID (UXXXXXXXX). When set, messages from this user
    /// are filtered in the listener as an additional guard against loops.
    pub bot_user_id: Option<String>,
    /// When true (default), channel/group messages only reach the agent if the
    /// bot is @mentioned or if the message is a thread reply. DMs always pass.
    /// Mirrors OpenClaw's `requireMention` default behaviour.
    pub mention_gating: bool,
    pub nats: NatsConfig,
    /// Port for the HTTP health check endpoint. Default: 8080.
    pub health_port: u16,
}

impl SlackBotConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let bot_token = env
            .var("SLACK_BOT_TOKEN")
            .expect("SLACK_BOT_TOKEN must be set");
        let app_token = env
            .var("SLACK_APP_TOKEN")
            .expect("SLACK_APP_TOKEN must be set");
        let bot_user_id = env.var("SLACK_BOT_USER_ID").ok();
        // Mention-gating mirrors OpenClaw's `requireMention` default: true.
        // Set SLACK_MENTION_GATING=false to respond to all channel messages.
        let mention_gating = env
            .var("SLACK_MENTION_GATING")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);
        let nats = NatsConfig::from_env(env);
        let health_port = env
            .var("HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);

        Self {
            bot_token,
            app_token,
            bot_user_id,
            mention_gating,
            nats,
            health_port,
        }
    }
}
