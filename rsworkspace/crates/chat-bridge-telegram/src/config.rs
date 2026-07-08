use acp_nats::{AcpPrefix, NatsConfig};
use anyhow::Context;
use std::path::PathBuf;
use trogon_std::env::ReadEnv;

pub struct BridgeConfig {
    pub acp: acp_nats::Config,
    /// Environment/tenant token for KV buckets and the durable consumer name.
    pub chat_prefix: String,
    /// JetStream stream the trogon-gateway Telegram source provisions.
    pub inbound_stream: String,
    pub bot_token: String,
    /// Endpoint account token; identifies which bot account on Telegram.
    pub bot_account: String,
    /// Agent every new conversation binds to (v1 routing policy: single agent).
    pub agent_id: String,
    /// Workspace the agent roots its sessions in; agent configuration, never
    /// a channel concern (see the architecture doc).
    pub agent_cwd: PathBuf,
    /// Telegram user ids seeded as principals at startup. Bootstrap only;
    /// ongoing administration mutates the KV buckets out of band.
    pub seed_users: Vec<i64>,
}

impl BridgeConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> anyhow::Result<Self> {
        let bot_token = env
            .var("TELEGRAM_BOT_TOKEN")
            .context("TELEGRAM_BOT_TOKEN not set")?;

        let chat_prefix = env.var("CHAT_PREFIX").unwrap_or_else(|_| "prod".to_string());
        let inbound_stream = env
            .var("TELEGRAM_INBOUND_STREAM")
            .unwrap_or_else(|_| "TELEGRAM".to_string());
        let bot_account = env
            .var("TELEGRAM_BOT_ACCOUNT")
            .unwrap_or_else(|_| "bot".to_string());
        let agent_id = env.var("CHAT_AGENT_ID").unwrap_or_else(|_| "default".to_string());
        let agent_cwd = env
            .var("CHAT_AGENT_CWD")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());

        let seed_users = match env.var("CHAT_SEED_TELEGRAM_USERS") {
            Ok(raw) => raw
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    s.trim()
                        .parse::<i64>()
                        .with_context(|| format!("invalid Telegram user id in CHAT_SEED_TELEGRAM_USERS: {s:?}"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            Err(_) => Vec::new(),
        };

        let raw_prefix = env
            .var(acp_nats::ENV_ACP_PREFIX)
            .unwrap_or_else(|_| acp_nats::DEFAULT_ACP_PREFIX.to_string());
        let acp_prefix = AcpPrefix::new(raw_prefix).context("invalid ACP prefix")?;
        let acp = acp_nats::Config::with_prefix(acp_prefix, NatsConfig::from_env(env));

        Ok(Self {
            acp,
            chat_prefix,
            inbound_stream,
            bot_token,
            bot_account,
            agent_id,
            agent_cwd,
            seed_users,
        })
    }
}
