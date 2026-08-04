#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;

use acp_nats::{AcpPrefix, NatsConfig};
use anyhow::Context;
use std::path::PathBuf;
use trogon_channel::CommandTriggers;
use trogon_nats::jetstream::ClaimBucket;
use trogon_std::env::ReadEnv;

/// A Telegram Bot API token that cannot be blank.
///
/// Blank is rejected because "unset" reaches a process as the empty string more
/// often than as an absent variable: Compose renders `"${TELEGRAM_BOT_TOKEN:-}"`
/// that way, and so does a Kubernetes secret reference to a missing key. Reading
/// that as present would boot the bridge on a token that only fails later, on
/// the first Bot API call, with nothing at startup to say why.
#[derive(Clone, PartialEq, Eq)]
pub struct BotToken(String);

#[derive(Debug, thiserror::Error)]
#[error("bot token is blank")]
pub struct BlankBotTokenError;

impl BotToken {
    /// Surrounding whitespace is trimmed rather than rejected: a token read
    /// from a file or a heredoc almost always arrives with a trailing newline,
    /// and Telegram would reject it with no hint as to which byte was wrong.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, BlankBotTokenError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(BlankBotTokenError);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Redacted, and deliberately without a `Display`: a config struct is exactly
/// the kind of value that ends up in a debug log.
impl std::fmt::Debug for BotToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BotToken(redacted)")
    }
}

/// Reads a variable, treating one that is present but blank as absent. Same
/// reason as [`BotToken`]: a deployment that renders an unset variable as the
/// empty string must fall back to the default rather than configure an empty
/// stream name or KV bucket prefix.
fn var<E: ReadEnv>(env: &E, key: &str) -> Option<String> {
    env.var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub struct BridgeConfig {
    pub acp: acp_nats::Config,
    /// Environment/tenant token for KV buckets and the durable consumer name.
    pub channel_prefix: String,
    /// JetStream stream the trogon-gateway Telegram source provisions.
    pub inbound_stream: String,
    /// Object-store bucket the gateway offloads oversized bodies to. Reading it
    /// is not optional: an update over the NATS max payload arrives as an empty
    /// body plus claim headers, and the bytes are only in the bucket.
    ///
    /// Deliberately not configurable. The gateway provisions and publishes to
    /// [`ClaimBucket::default`] unconditionally and stamps that name into every
    /// claim's headers, so any other value here can only be wrong: the bucket
    /// would be missing at boot, or present and rejected as a `BucketMismatch`
    /// when a real claim arrives. A shared constant is what keeps the publisher
    /// and the resolver in agreement; an env knob on one side cannot.
    pub claim_bucket: ClaimBucket,
    pub bot_token: BotToken,
    /// Endpoint account token; identifies which bot account on Telegram.
    pub bot_account: String,
    /// Agent every new conversation binds to; the routing policy is one agent.
    pub agent_id: String,
    /// Workspace the agent roots its sessions in; agent configuration, never
    /// a channel concern (see the architecture doc).
    pub agent_cwd: PathBuf,
    /// Telegram user ids seeded as principals at startup. Bootstrap only;
    /// ongoing administration mutates the KV buckets out of band.
    pub seed_users: Vec<i64>,
    /// Message text the bridge answers to itself. Configurable so a deployment
    /// can move out of the way of an agent that advertises the same triggers,
    /// or set none at all to forward everything.
    pub command_triggers: CommandTriggers,
}

impl BridgeConfig {
    pub fn from_env<E: ReadEnv>(env: &E) -> anyhow::Result<Self> {
        let bot_token =
            BotToken::new(env.var("TELEGRAM_BOT_TOKEN").unwrap_or_default()).context("TELEGRAM_BOT_TOKEN not set")?;

        let channel_prefix = var(env, "CHANNEL_PREFIX").unwrap_or_else(|| "prod".to_string());
        let inbound_stream = var(env, "TELEGRAM_INBOUND_STREAM").unwrap_or_else(|| "TELEGRAM".to_string());
        let bot_account = var(env, "TELEGRAM_BOT_ACCOUNT").unwrap_or_else(|| "bot".to_string());
        let agent_id = var(env, "CHANNEL_AGENT_ID").unwrap_or_else(|| "default".to_string());
        let agent_cwd = var(env, "CHANNEL_AGENT_CWD").map_or_else(std::env::temp_dir, PathBuf::from);

        let seed_users = match var(env, "CHANNEL_SEED_TELEGRAM_USERS") {
            Some(raw) => raw
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    s.trim()
                        .parse::<i64>()
                        .with_context(|| format!("invalid Telegram user id in CHANNEL_SEED_TELEGRAM_USERS: {s:?}"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            None => Vec::new(),
        };

        // Read directly rather than through `var`: this is the one setting where
        // blank is a value rather than an omission. An empty trigger list means
        // "recognize nothing, forward everything", which a deployment can only
        // ask for by setting the variable to empty.
        let command_triggers = match env.var("CHANNEL_NEW_SESSION_TRIGGERS") {
            Ok(raw) => CommandTriggers::new(
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from),
            )
            .context("invalid CHANNEL_NEW_SESSION_TRIGGERS")?,
            Err(_) => CommandTriggers::default(),
        };

        let raw_prefix = var(env, acp_nats::ENV_ACP_PREFIX).unwrap_or_else(|| acp_nats::DEFAULT_ACP_PREFIX.to_string());
        let acp_prefix = AcpPrefix::new(raw_prefix).context("invalid ACP prefix")?;
        let acp = acp_nats::Config::with_prefix(acp_prefix, NatsConfig::from_env(env));

        Ok(Self {
            acp,
            channel_prefix,
            inbound_stream,
            claim_bucket: ClaimBucket::default(),
            bot_token,
            bot_account,
            agent_id,
            agent_cwd,
            seed_users,
            command_triggers,
        })
    }
}
