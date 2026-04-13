use std::fmt;
use std::path::Path;

use confique::Config;
#[cfg(test)]
use trogon_nats::NatsAuth;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{NatsToken, SubjectTokenViolation};
use trogon_service_config::{NatsArgs, NatsConfigSection, load_config, resolve_nats};
use trogon_source_discord::config::DiscordBotToken;
use trogon_source_github::config::GitHubWebhookSecret;
use trogon_source_gitlab::config::GitLabWebhookSecret;
use trogon_source_incidentio::config::IncidentioConfig as IncidentioSourceConfig;
use trogon_source_incidentio::incidentio_signing_secret::IncidentioSigningSecret;
use trogon_source_linear::config::LinearWebhookSecret;
use trogon_source_notion::NotionVerificationToken;
use trogon_source_slack::config::SlackSigningSecret;
use trogon_source_telegram::config::TelegramWebhookSecret;
use trogon_std::{NonZeroDuration, ZeroDuration};

#[derive(Debug)]
pub enum ConfigValidationError {
    InvalidField {
        source: &'static str,
        field: &'static str,
        error: Box<dyn std::error::Error + 'static>,
    },
    InvalidSubjectToken {
        source: &'static str,
        field: &'static str,
        violation: SubjectTokenViolation,
    },
}

impl ConfigValidationError {
    fn invalid<E>(source: &'static str, field: &'static str, error: E) -> Self
    where
        E: std::error::Error + 'static,
    {
        Self::InvalidField {
            source,
            field,
            error: Box::new(error),
        }
    }

    fn invalid_subject_token(
        source: &'static str,
        field: &'static str,
        violation: SubjectTokenViolation,
    ) -> Self {
        Self::InvalidSubjectToken {
            source,
            field,
            violation,
        }
    }

    #[cfg(test)]
    fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField {
                source,
                field,
                error,
            } => {
                if error.downcast_ref::<ZeroDuration>().is_some() {
                    write!(f, "{source}: {field} must not be zero")
                } else {
                    write!(f, "{source}: invalid {field}: {error}")
                }
            }
            Self::InvalidSubjectToken {
                source,
                field,
                violation,
            } => write!(f, "{source}: invalid {field}: {violation:?}"),
        }
    }
}

impl std::error::Error for ConfigValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidField { error, .. } => Some(error.as_ref()),
            Self::InvalidSubjectToken { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Load(confique::Error),
    Validation(Vec<ConfigValidationError>),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(e) => write!(f, "failed to load config: {e}"),
            Self::Validation(errors) => {
                writeln!(f, "config validation errors:")?;
                for e in errors {
                    writeln!(f, "  - {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Config)]
struct GatewayConfig {
    #[config(nested)]
    http_server: HttpServerConfig,
    #[config(nested)]
    nats: NatsConfigSection,
    #[config(nested)]
    sources: SourcesConfig,
}

#[derive(Config)]
struct HttpServerConfig {
    #[config(env = "TROGON_GATEWAY_PORT", default = 8080)]
    port: u16,
}

#[derive(Config)]
struct SourcesConfig {
    #[config(nested)]
    github: GithubConfig,
    #[config(nested)]
    discord: DiscordConfig,
    #[config(nested)]
    slack: SlackConfig,
    #[config(nested)]
    telegram: TelegramConfig,
    #[config(nested)]
    gitlab: GitlabConfig,
    #[config(nested)]
    incidentio: IncidentioConfig,
    #[config(nested)]
    linear: LinearConfig,
    #[config(nested)]
    notion: NotionConfig,
}

#[derive(Config)]
struct GithubConfig {
    #[config(env = "TROGON_SOURCE_GITHUB_WEBHOOK_SECRET")]
    webhook_secret: Option<String>,
    #[config(env = "TROGON_SOURCE_GITHUB_SUBJECT_PREFIX", default = "github")]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_GITHUB_STREAM_NAME", default = "GITHUB")]
    stream_name: String,
    #[config(env = "TROGON_SOURCE_GITHUB_STREAM_MAX_AGE_SECS", default = 604_800)]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_GITHUB_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
}

#[derive(Config)]
struct DiscordConfig {
    #[config(env = "TROGON_SOURCE_DISCORD_BOT_TOKEN")]
    bot_token: Option<String>,
    #[config(env = "TROGON_SOURCE_DISCORD_GATEWAY_INTENTS")]
    gateway_intents: Option<String>,
    #[config(env = "TROGON_SOURCE_DISCORD_SUBJECT_PREFIX", default = "discord")]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_DISCORD_STREAM_NAME", default = "DISCORD")]
    stream_name: String,
    #[config(env = "TROGON_SOURCE_DISCORD_STREAM_MAX_AGE_SECS", default = 604_800)]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_DISCORD_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
}

#[derive(Config)]
struct SlackConfig {
    #[config(env = "TROGON_SOURCE_SLACK_SIGNING_SECRET")]
    signing_secret: Option<String>,
    #[config(env = "TROGON_SOURCE_SLACK_SUBJECT_PREFIX", default = "slack")]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_SLACK_STREAM_NAME", default = "SLACK")]
    stream_name: String,
    #[config(env = "TROGON_SOURCE_SLACK_STREAM_MAX_AGE_SECS", default = 604_800)]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_SLACK_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
    #[config(env = "TROGON_SOURCE_SLACK_TIMESTAMP_MAX_DRIFT_SECS", default = 300)]
    timestamp_max_drift_secs: u64,
}

#[derive(Config)]
struct TelegramConfig {
    #[config(env = "TROGON_SOURCE_TELEGRAM_WEBHOOK_SECRET")]
    webhook_secret: Option<String>,
    #[config(env = "TROGON_SOURCE_TELEGRAM_SUBJECT_PREFIX", default = "telegram")]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_TELEGRAM_STREAM_NAME", default = "TELEGRAM")]
    stream_name: String,
    #[config(env = "TROGON_SOURCE_TELEGRAM_STREAM_MAX_AGE_SECS", default = 604_800)]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_TELEGRAM_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
}

#[derive(Config)]
struct GitlabConfig {
    #[config(env = "TROGON_SOURCE_GITLAB_WEBHOOK_SECRET")]
    webhook_secret: Option<String>,
    #[config(env = "TROGON_SOURCE_GITLAB_SUBJECT_PREFIX", default = "gitlab")]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_GITLAB_STREAM_NAME", default = "GITLAB")]
    stream_name: String,
    #[config(env = "TROGON_SOURCE_GITLAB_STREAM_MAX_AGE_SECS", default = 604_800)]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_GITLAB_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
}

#[derive(Config)]
struct LinearConfig {
    #[config(env = "TROGON_SOURCE_LINEAR_WEBHOOK_SECRET")]
    webhook_secret: Option<String>,
    #[config(env = "TROGON_SOURCE_LINEAR_SUBJECT_PREFIX", default = "linear")]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_LINEAR_STREAM_NAME", default = "LINEAR")]
    stream_name: String,
    #[config(env = "TROGON_SOURCE_LINEAR_STREAM_MAX_AGE_SECS", default = 604_800)]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_LINEAR_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
    #[config(env = "TROGON_SOURCE_LINEAR_TIMESTAMP_TOLERANCE_SECS", default = 60)]
    timestamp_tolerance_secs: u64,
}

#[derive(Config)]
struct IncidentioConfig {
    #[config(env = "TROGON_SOURCE_INCIDENTIO_SIGNING_SECRET")]
    signing_secret: Option<String>,
    #[config(
        env = "TROGON_SOURCE_INCIDENTIO_SUBJECT_PREFIX",
        default = "incidentio"
    )]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_INCIDENTIO_STREAM_NAME", default = "INCIDENTIO")]
    stream_name: String,
    #[config(
        env = "TROGON_SOURCE_INCIDENTIO_STREAM_MAX_AGE_SECS",
        default = 604_800
    )]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_INCIDENTIO_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
    #[config(
        env = "TROGON_SOURCE_INCIDENTIO_TIMESTAMP_TOLERANCE_SECS",
        default = 300
    )]
    timestamp_tolerance_secs: u64,
}

#[derive(Config)]
struct NotionConfig {
    #[config(env = "TROGON_SOURCE_NOTION_VERIFICATION_TOKEN")]
    verification_token: Option<String>,
    #[config(env = "TROGON_SOURCE_NOTION_SUBJECT_PREFIX", default = "notion")]
    subject_prefix: String,
    #[config(env = "TROGON_SOURCE_NOTION_STREAM_NAME", default = "NOTION")]
    stream_name: String,
    #[config(env = "TROGON_SOURCE_NOTION_STREAM_MAX_AGE_SECS", default = 604_800)]
    stream_max_age_secs: u64,
    #[config(env = "TROGON_SOURCE_NOTION_NATS_ACK_TIMEOUT_SECS", default = 10)]
    nats_ack_timeout_secs: u64,
}

pub struct ResolvedHttpServerConfig {
    pub port: u16,
}

pub struct ResolvedConfig {
    pub http_server: ResolvedHttpServerConfig,
    pub nats: trogon_nats::NatsConfig,
    pub github: Option<trogon_source_github::GithubConfig>,
    pub discord: Option<trogon_source_discord::DiscordConfig>,
    pub slack: Option<trogon_source_slack::SlackConfig>,
    pub telegram: Option<trogon_source_telegram::TelegramSourceConfig>,
    pub gitlab: Option<trogon_source_gitlab::GitlabConfig>,
    pub incidentio: Option<trogon_source_incidentio::IncidentioConfig>,
    pub linear: Option<trogon_source_linear::LinearConfig>,
    pub notion: Option<trogon_source_notion::NotionConfig>,
}

impl ResolvedConfig {
    pub fn has_any_source(&self) -> bool {
        self.github.is_some()
            || self.discord.is_some()
            || self.slack.is_some()
            || self.telegram.is_some()
            || self.gitlab.is_some()
            || self.incidentio.is_some()
            || self.linear.is_some()
            || self.notion.is_some()
    }
}

#[cfg(test)]
pub fn load(config_path: Option<&Path>) -> Result<ResolvedConfig, ConfigError> {
    load_with_overrides(config_path, &NatsArgs::default())
}

pub fn load_with_overrides(
    config_path: Option<&Path>,
    nats_overrides: &NatsArgs,
) -> Result<ResolvedConfig, ConfigError> {
    let cfg = load_config::<GatewayConfig>(config_path).map_err(ConfigError::Load)?;
    resolve(cfg, nats_overrides)
}

fn resolve(cfg: GatewayConfig, nats_overrides: &NatsArgs) -> Result<ResolvedConfig, ConfigError> {
    let nats = resolve_nats(&cfg.nats, nats_overrides);
    let mut errors = Vec::new();

    let github = resolve_github(cfg.sources.github, &mut errors);
    let discord = resolve_discord(cfg.sources.discord, &mut errors);
    let slack = resolve_slack(cfg.sources.slack, &mut errors);
    let telegram = resolve_telegram(cfg.sources.telegram, &mut errors);
    let gitlab = resolve_gitlab(cfg.sources.gitlab, &mut errors);
    let incidentio = resolve_incidentio(cfg.sources.incidentio, &mut errors);
    let linear = resolve_linear(cfg.sources.linear, &mut errors);
    let notion = resolve_notion(cfg.sources.notion, &mut errors);

    if !errors.is_empty() {
        return Err(ConfigError::Validation(errors));
    }

    Ok(ResolvedConfig {
        http_server: ResolvedHttpServerConfig {
            port: cfg.http_server.port,
        },
        nats,
        github,
        discord,
        slack,
        telegram,
        gitlab,
        incidentio,
        linear,
        notion,
    })
}

fn resolve_github(
    section: GithubConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<trogon_source_github::GithubConfig> {
    let secret_str = section.webhook_secret?;
    let webhook_secret = match GitHubWebhookSecret::new(secret_str) {
        Ok(s) => s,
        Err(e) => {
            errors.push(ConfigValidationError::invalid(
                "github",
                "webhook_secret",
                e,
            ));
            return None;
        }
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "github",
                "subject_prefix",
                e,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "github",
                "stream_name",
                e,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "github",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "github",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(trogon_source_github::GithubConfig {
        webhook_secret,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
    })
}

fn resolve_discord(
    section: DiscordConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<trogon_source_discord::DiscordConfig> {
    let token_str = section.bot_token.as_deref()?;
    let bot_token = match DiscordBotToken::new(token_str) {
        Ok(s) => s,
        Err(e) => {
            errors.push(ConfigValidationError::invalid("discord", "bot_token", e));
            return None;
        }
    };

    let intents = if let Some(s) = section.gateway_intents.as_deref().filter(|s| !s.is_empty()) {
        match trogon_source_discord::config::parse_gateway_intents(s) {
            Ok(i) => i,
            Err(e) => {
                errors.push(ConfigValidationError::invalid(
                    "discord",
                    "gateway_intents",
                    e,
                ));
                return None;
            }
        }
    } else {
        trogon_source_discord::config::default_intents()
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "discord",
                "subject_prefix",
                e,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "discord",
                "stream_name",
                e,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "discord",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "discord",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(trogon_source_discord::DiscordConfig {
        bot_token,
        intents,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
    })
}

fn resolve_slack(
    section: SlackConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<trogon_source_slack::SlackConfig> {
    let secret_str = section.signing_secret?;
    let signing_secret = match SlackSigningSecret::new(secret_str) {
        Ok(s) => s,
        Err(e) => {
            errors.push(ConfigValidationError::invalid("slack", "signing_secret", e));
            return None;
        }
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "slack",
                "subject_prefix",
                e,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "slack",
                "stream_name",
                e,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "slack",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let timestamp_max_drift = match NonZeroDuration::from_secs(section.timestamp_max_drift_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "slack",
                "timestamp_max_drift_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "slack",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(trogon_source_slack::SlackConfig {
        signing_secret,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
        timestamp_max_drift,
    })
}

fn resolve_telegram(
    section: TelegramConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<trogon_source_telegram::TelegramSourceConfig> {
    let secret_str = section.webhook_secret?;
    let webhook_secret = match TelegramWebhookSecret::new(secret_str) {
        Ok(s) => s,
        Err(e) => {
            errors.push(ConfigValidationError::invalid(
                "telegram",
                "webhook_secret",
                e,
            ));
            return None;
        }
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "telegram",
                "subject_prefix",
                e,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "telegram",
                "stream_name",
                e,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "telegram",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "telegram",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(trogon_source_telegram::TelegramSourceConfig {
        webhook_secret,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
    })
}

fn resolve_gitlab(
    section: GitlabConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<trogon_source_gitlab::GitlabConfig> {
    let webhook_secret_str = section.webhook_secret?;
    let webhook_secret = match GitLabWebhookSecret::new(webhook_secret_str) {
        Ok(s) => s,
        Err(e) => {
            errors.push(ConfigValidationError::invalid(
                "gitlab",
                "webhook_secret",
                e,
            ));
            return None;
        }
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "gitlab",
                "subject_prefix",
                e,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "gitlab",
                "stream_name",
                e,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "gitlab",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "gitlab",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(trogon_source_gitlab::GitlabConfig {
        webhook_secret,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
    })
}

fn resolve_linear(
    section: LinearConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<trogon_source_linear::LinearConfig> {
    let secret_str = section.webhook_secret?;
    let webhook_secret = match LinearWebhookSecret::new(secret_str) {
        Ok(s) => s,
        Err(e) => {
            errors.push(ConfigValidationError::invalid(
                "linear",
                "webhook_secret",
                e,
            ));
            return None;
        }
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "linear",
                "subject_prefix",
                e,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(t) => t,
        Err(e) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "linear",
                "stream_name",
                e,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "linear",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "linear",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(trogon_source_linear::LinearConfig {
        webhook_secret,
        subject_prefix,
        stream_name,
        stream_max_age,
        timestamp_tolerance: NonZeroDuration::from_secs(section.timestamp_tolerance_secs).ok(),
        nats_ack_timeout,
    })
}

fn resolve_incidentio(
    section: IncidentioConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<IncidentioSourceConfig> {
    let signing_secret_str = section.signing_secret?;
    let signing_secret = match IncidentioSigningSecret::new(signing_secret_str) {
        Ok(secret) => secret,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "incidentio",
                "signing_secret",
                err,
            ));
            return None;
        }
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(token) => token,
        Err(err) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "incidentio",
                "subject_prefix",
                err,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(token) => token,
        Err(err) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "incidentio",
                "stream_name",
                err,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(duration) => duration,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "incidentio",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let timestamp_tolerance = match NonZeroDuration::from_secs(section.timestamp_tolerance_secs) {
        Ok(duration) => duration,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "incidentio",
                "timestamp_tolerance_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "incidentio",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(IncidentioSourceConfig {
        signing_secret,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
        timestamp_tolerance,
    })
}

fn resolve_notion(
    section: NotionConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<trogon_source_notion::NotionConfig> {
    let verification_token = match section.verification_token {
        Some(token) => token,
        None => {
            return None;
        }
    };

    let verification_token = match NotionVerificationToken::new(verification_token) {
        Ok(token) => token,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "notion",
                "verification_token",
                err,
            ));
            return None;
        }
    };

    let subject_prefix = match NatsToken::new(section.subject_prefix) {
        Ok(token) => token,
        Err(err) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "notion",
                "subject_prefix",
                err,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(section.stream_name) {
        Ok(token) => token,
        Err(err) => {
            errors.push(ConfigValidationError::invalid_subject_token(
                "notion",
                "stream_name",
                err,
            ));
            return None;
        }
    };

    let nats_ack_timeout = match NonZeroDuration::from_secs(section.nats_ack_timeout_secs) {
        Ok(duration) => duration,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "notion",
                "nats_ack_timeout_secs",
                err,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(section.stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid(
                "notion",
                "stream_max_age_secs",
                err,
            ));
            return None;
        }
    };

    Some(trogon_source_notion::NotionConfig {
        verification_token,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::fmt;
    use std::io::Write;

    fn write_toml(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".toml")
            .tempfile()
            .expect("failed to create temp file");
        f.write_all(content.as_bytes())
            .expect("failed to write toml");
        f.flush().expect("failed to flush");
        f
    }

    fn minimal_toml() -> String {
        String::new()
    }

    fn github_toml(secret: &str) -> String {
        format!(
            r#"
[sources.github]
webhook_secret = "{secret}"
"#
        )
    }

    fn discord_gateway_toml(bot_token: &str) -> String {
        format!(
            r#"
[sources.discord]
bot_token = "{bot_token}"
"#
        )
    }

    fn slack_toml(secret: &str) -> String {
        format!(
            r#"
[sources.slack]
signing_secret = "{secret}"
"#
        )
    }

    fn telegram_toml(secret: &str) -> String {
        format!(
            r#"
[sources.telegram]
webhook_secret = "{secret}"
"#
        )
    }

    fn gitlab_toml(secret: &str) -> String {
        format!(
            r#"
[sources.gitlab]
webhook_secret = "{secret}"
"#
        )
    }

    fn linear_toml(secret: &str) -> String {
        format!(
            r#"
[sources.linear]
webhook_secret = "{secret}"
"#
        )
    }

    fn incidentio_toml(secret: &str) -> String {
        format!(
            r#"
[sources.incidentio]
signing_secret = "{secret}"
"#
        )
    }

    fn notion_toml(token: &str) -> String {
        format!(
            r#"
[sources.notion]
verification_token = "{token}"
"#
        )
    }

    fn incidentio_valid_test_secret() -> String {
        ["whsec_", "dGVzdC1zZWNyZXQ="].concat()
    }

    #[derive(Debug)]
    struct DummyConfigError;

    impl fmt::Display for DummyConfigError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("dummy config error")
        }
    }

    impl Error for DummyConfigError {}

    fn nats_toml_with_creds(creds: &str) -> String {
        format!(
            r#"
[nats]
creds = "{creds}"
"#
        )
    }

    fn nats_toml_with_nkey(nkey: &str) -> String {
        format!(
            r#"
[nats]
nkey = "{nkey}"
"#
        )
    }

    fn nats_toml_with_user_password(user: &str, password: &str) -> String {
        format!(
            r#"
[nats]
user = "{user}"
password = "{password}"
"#
        )
    }

    fn nats_toml_with_token(token: &str) -> String {
        format!(
            r#"
[nats]
token = "{token}"
"#
        )
    }

    #[test]
    fn has_any_source_returns_false_when_nothing_configured() {
        let f = write_toml(&minimal_toml());
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.has_any_source());
    }

    #[test]
    fn github_resolves_with_valid_secret() {
        let f = write_toml(&github_toml("my-gh-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.github.is_some());
        assert!(cfg.has_any_source());
    }

    #[test]
    fn discord_gateway_resolves_with_valid_token() {
        let f = write_toml(&discord_gateway_toml("Bot my-bot-token"));
        let cfg = load(Some(f.path())).expect("load failed");
        let discord = cfg.discord.as_ref().expect("discord should be Some");
        assert_eq!(discord.bot_token.as_str(), "Bot my-bot-token");
    }

    #[test]
    fn discord_gateway_with_intents() {
        let toml = r#"
[sources.discord]
bot_token = "Bot my-bot-token"
gateway_intents = "guilds,guild_messages"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.discord.is_some());
    }

    #[test]
    fn discord_gateway_with_invalid_intents() {
        let toml = r#"
[sources.discord]
bot_token = "Bot my-bot-token"
gateway_intents = "bogus_intent"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(matches!(result, Err(ConfigError::Validation(_))));
    }

    #[test]
    fn discord_missing_bot_token_returns_none() {
        let toml = r#"
[sources.discord]
gateway_intents = "guilds,guild_messages"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.discord.is_none());
    }

    #[test]
    fn discord_gateway_empty_bot_token() {
        let toml = r#"
[sources.discord]
bot_token = ""
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("invalid bot_token")))
        );
    }

    #[test]
    fn slack_resolves_with_valid_secret() {
        let f = write_toml(&slack_toml("slack-signing-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.slack.is_some());
    }

    #[test]
    fn telegram_resolves_with_valid_secret() {
        let f = write_toml(&telegram_toml("telegram-webhook-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.telegram.is_some());
    }

    #[test]
    fn gitlab_resolves_with_valid_secret() {
        let f = write_toml(&gitlab_toml("gitlab-webhook-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.gitlab.is_some());
    }

    #[test]
    fn linear_resolves_with_valid_secret() {
        let f = write_toml(&linear_toml("linear-webhook-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.linear.is_some());
    }

    #[test]
    fn incidentio_resolves_with_valid_secret() {
        let f = write_toml(&incidentio_toml(&incidentio_valid_test_secret()));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.incidentio.is_some());
    }

    #[test]
    fn notion_resolves_with_valid_token() {
        let f = write_toml(&notion_toml("notion-verification-token-example"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.notion.is_some());
    }

    #[test]
    fn notion_missing_token_returns_none() {
        let toml = r#"
[sources.notion]
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.notion.is_none());
    }

    #[test]
    fn linear_with_zero_timestamp_tolerance() {
        let toml = r#"
[sources.linear]
webhook_secret = "linear-secret"
timestamp_tolerance_secs = 0
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let linear = cfg.linear.as_ref().expect("linear should be Some");
        assert!(linear.timestamp_tolerance.is_none());
    }

    #[test]
    fn github_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.github]
webhook_secret = "gh-secret"
nats_ack_timeout_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn github_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.github]
webhook_secret = "gh-secret"
stream_max_age_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn slack_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.slack]
signing_secret = "slack-secret"
nats_ack_timeout_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn slack_zero_timestamp_max_drift_is_error() {
        let toml = r#"
[sources.slack]
signing_secret = "slack-secret"
timestamp_max_drift_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("timestamp_max_drift_secs must not be zero")))
        );
    }

    #[test]
    fn slack_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.slack]
signing_secret = "slack-secret"
stream_max_age_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn telegram_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.telegram]
webhook_secret = "tg-secret"
nats_ack_timeout_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn telegram_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.telegram]
webhook_secret = "tg-secret"
stream_max_age_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn gitlab_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.gitlab]
webhook_secret = "gl-secret"
nats_ack_timeout_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn gitlab_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.gitlab]
webhook_secret = "gl-secret"
stream_max_age_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn linear_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.linear]
webhook_secret = "lin-secret"
nats_ack_timeout_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn linear_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.linear]
webhook_secret = "lin-secret"
stream_max_age_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn discord_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.discord]
bot_token = "Bot token"
nats_ack_timeout_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn discord_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.discord]
bot_token = "Bot token"
stream_max_age_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn github_invalid_subject_prefix() {
        let toml = r#"
[sources.github]
webhook_secret = "gh-secret"
subject_prefix = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("subject_prefix")))
        );
    }

    #[test]
    fn github_invalid_stream_name() {
        let toml = r#"
[sources.github]
webhook_secret = "gh-secret"
stream_name = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name")))
        );
    }

    #[test]
    fn nats_default_is_no_auth() {
        let f = write_toml(&minimal_toml());
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::None));
    }

    #[test]
    fn nats_credentials_auth() {
        let f = write_toml(&nats_toml_with_creds("/path/to/creds"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::Credentials(_)));
    }

    #[test]
    fn nats_nkey_auth() {
        let f = write_toml(&nats_toml_with_nkey("SUAIBDPBAUTW"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::NKey(_)));
    }

    #[test]
    fn nats_user_password_auth() {
        let f = write_toml(&nats_toml_with_user_password("myuser", "mypass"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::UserPassword { .. }));
    }

    #[test]
    fn nats_token_auth() {
        let f = write_toml(&nats_toml_with_token("mytoken"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::Token(_)));
    }

    #[test]
    fn nats_creds_takes_priority_over_token() {
        let toml = r#"
[nats]
creds = "/path/to/creds"
token = "mytoken"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::Credentials(_)));
    }

    #[test]
    fn nats_nkey_takes_priority_over_user_password() {
        let toml = r#"
[nats]
nkey = "SUAIBDPBAUTW"
user = "myuser"
password = "mypass"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::NKey(_)));
    }

    #[test]
    fn nats_user_password_takes_priority_over_token() {
        let toml = r#"
[nats]
user = "myuser"
password = "mypass"
token = "mytoken"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::UserPassword { .. }));
    }

    #[test]
    fn nats_empty_creds_falls_through() {
        let toml = r#"
[nats]
creds = ""
token = "mytoken"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::Token(_)));
    }

    #[test]
    fn nats_url_comma_separated() {
        let toml = r#"
[nats]
url = "host1:4222, host2:4222, host3:4222"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert_eq!(cfg.nats.servers.len(), 3);
    }

    #[test]
    fn nats_user_without_password_falls_through() {
        let toml = r#"
[nats]
user = "myuser"
token = "mytoken"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(matches!(cfg.nats.auth, NatsAuth::Token(_)));
    }

    #[test]
    fn load_with_overrides_prefers_cli_nats_values() {
        let toml = r#"
[nats]
url = "file1:4222,file2:4222"
token = "file-token"
"#;
        let f = write_toml(toml);
        let cfg = load_with_overrides(
            Some(f.path()),
            &NatsArgs {
                nats_url: Some("override:4222".to_string()),
                nats_token: Some("override-token".to_string()),
                ..Default::default()
            },
        )
        .expect("load failed");

        assert_eq!(cfg.nats.servers, vec!["override:4222"]);
        assert!(matches!(cfg.nats.auth, NatsAuth::Token(ref token) if token == "override-token"));
    }

    #[test]
    fn load_with_overrides_keeps_auth_priority() {
        let toml = r#"
[nats]
token = "file-token"
"#;
        let f = write_toml(toml);
        let cfg = load_with_overrides(
            Some(f.path()),
            &NatsArgs {
                nats_creds: Some("/path/to/override.creds".to_string()),
                nats_token: Some("override-token".to_string()),
                ..Default::default()
            },
        )
        .expect("load failed");

        assert!(
            matches!(cfg.nats.auth, NatsAuth::Credentials(ref path) if path == std::path::Path::new("/path/to/override.creds"))
        );
    }

    #[test]
    fn config_error_display_load() {
        let f = write_toml("this is not { valid toml");
        let result = load(Some(f.path()));
        assert!(matches!(result, Err(ConfigError::Load(_))));
        let err = result.err().unwrap();
        let display = format!("{err}");
        assert!(display.contains("failed to load config"));
    }

    #[test]
    fn config_error_display_validation() {
        let err = ConfigError::Validation(vec![
            ConfigValidationError::invalid("github", "stream_max_age_secs", ZeroDuration),
            ConfigValidationError::invalid_subject_token(
                "discord",
                "subject_prefix",
                SubjectTokenViolation::InvalidCharacter('.'),
            ),
        ]);
        let display = format!("{err}");
        assert!(display.contains("config validation errors:"));
        assert!(display.contains("github: stream_max_age_secs must not be zero"));
        assert!(display.contains("discord: invalid subject_prefix: InvalidCharacter('.')"));
    }

    #[test]
    fn config_validation_error_invalid_field_preserves_source() {
        let err = ConfigValidationError::invalid("incidentio", "signing_secret", DummyConfigError);

        assert_eq!(
            err.to_string(),
            "incidentio: invalid signing_secret: dummy config error"
        );
        assert!(err.source().is_some());
    }

    #[test]
    fn config_validation_error_invalid_subject_token_has_no_source() {
        let err = ConfigValidationError::invalid_subject_token(
            "incidentio",
            "subject_prefix",
            SubjectTokenViolation::InvalidCharacter('.'),
        );

        assert_eq!(
            err.to_string(),
            "incidentio: invalid subject_prefix: InvalidCharacter('.')"
        );
        assert!(err.source().is_none());
    }

    #[test]
    fn config_error_is_std_error() {
        let err = ConfigError::Validation(vec![ConfigValidationError::invalid(
            "github",
            "stream_max_age_secs",
            ZeroDuration,
        )]);
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn http_server_default_port() {
        let f = write_toml(&minimal_toml());
        let cfg = load(Some(f.path())).expect("load failed");
        assert_eq!(cfg.http_server.port, 8080);
    }

    #[test]
    fn http_server_custom_port() {
        let toml = r#"
[http_server]
port = 9090
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert_eq!(cfg.http_server.port, 9090);
    }

    #[test]
    fn github_empty_secret_is_invalid() {
        let f = write_toml(&github_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github: invalid webhook_secret")))
        );
    }

    #[test]
    fn slack_empty_secret_is_invalid() {
        let f = write_toml(&slack_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack: invalid signing_secret")))
        );
    }

    #[test]
    fn telegram_empty_secret_is_invalid() {
        let f = write_toml(&telegram_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram: invalid webhook_secret")))
        );
    }

    #[test]
    fn gitlab_empty_secret_is_invalid() {
        let f = write_toml(&gitlab_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("gitlab: invalid webhook_secret")))
        );
    }

    #[test]
    fn linear_empty_secret_is_invalid() {
        let f = write_toml(&linear_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("linear: invalid webhook_secret")))
        );
    }

    #[test]
    fn incidentio_empty_secret_is_invalid() {
        let f = write_toml(&incidentio_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: invalid signing_secret")))
        );
    }

    #[test]
    fn notion_empty_verification_token_is_invalid() {
        let f = write_toml(&notion_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion: invalid verification_token")))
        );
    }

    #[test]
    fn incidentio_invalid_secret_is_invalid() {
        let f = write_toml(&incidentio_toml("whsec_not-base64!"));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: invalid signing_secret")))
        );
    }

    #[test]
    fn incidentio_secret_without_prefix_is_invalid() {
        let f = write_toml(&incidentio_toml("dGVzdC1zZWNyZXQ="));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: invalid signing_secret")))
        );
    }

    #[test]
    fn discord_invalid_subject_prefix() {
        let toml = r#"
[sources.discord]
bot_token = "Bot token"
subject_prefix = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("subject_prefix")))
        );
    }

    #[test]
    fn discord_invalid_stream_name() {
        let toml = r#"
[sources.discord]
bot_token = "Bot token"
stream_name = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name")))
        );
    }

    #[test]
    fn slack_invalid_subject_prefix() {
        let toml = r#"
[sources.slack]
signing_secret = "slack-secret"
subject_prefix = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("subject_prefix")))
        );
    }

    #[test]
    fn telegram_invalid_subject_prefix() {
        let toml = r#"
[sources.telegram]
webhook_secret = "tg-secret"
subject_prefix = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("subject_prefix")))
        );
    }

    #[test]
    fn gitlab_invalid_subject_prefix() {
        let toml = r#"
[sources.gitlab]
webhook_secret = "gl-secret"
subject_prefix = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("subject_prefix")))
        );
    }

    #[test]
    fn linear_invalid_subject_prefix() {
        let toml = r#"
[sources.linear]
webhook_secret = "lin-secret"
subject_prefix = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("subject_prefix")))
        );
    }

    #[test]
    fn incidentio_invalid_subject_prefix() {
        let toml = format!(
            r#"
[sources.incidentio]
signing_secret = "{}"
subject_prefix = "has.dots"
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: invalid subject_prefix")))
        );
    }

    #[test]
    fn notion_invalid_subject_prefix() {
        let toml = r#"
[sources.notion]
verification_token = "notion-verification-token-example"
subject_prefix = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion: invalid subject_prefix")))
        );
    }

    #[test]
    fn slack_invalid_stream_name() {
        let toml = r#"
[sources.slack]
signing_secret = "slack-secret"
stream_name = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name")))
        );
    }

    #[test]
    fn telegram_invalid_stream_name() {
        let toml = r#"
[sources.telegram]
webhook_secret = "tg-secret"
stream_name = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name")))
        );
    }

    #[test]
    fn gitlab_invalid_stream_name() {
        let toml = r#"
[sources.gitlab]
webhook_secret = "gl-secret"
stream_name = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name")))
        );
    }

    #[test]
    fn linear_invalid_stream_name() {
        let toml = r#"
[sources.linear]
webhook_secret = "lin-secret"
stream_name = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name")))
        );
    }

    #[test]
    fn incidentio_invalid_stream_name() {
        let toml = format!(
            r#"
[sources.incidentio]
signing_secret = "{}"
stream_name = "has.dots"
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: invalid stream_name")))
        );
    }

    #[test]
    fn notion_invalid_stream_name() {
        let toml = r#"
[sources.notion]
verification_token = "notion-verification-token-example"
stream_name = "has.dots"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion: invalid stream_name")))
        );
    }

    #[test]
    fn incidentio_zero_nats_ack_timeout_is_error() {
        let toml = format!(
            r#"
[sources.incidentio]
signing_secret = "{}"
nats_ack_timeout_secs = 0
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn incidentio_zero_stream_max_age_is_error() {
        let toml = format!(
            r#"
[sources.incidentio]
signing_secret = "{}"
stream_max_age_secs = 0
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn notion_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.notion]
verification_token = "notion-verification-token-example"
nats_ack_timeout_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion: nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn notion_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.notion]
verification_token = "notion-verification-token-example"
stream_max_age_secs = 0
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion: stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn incidentio_zero_timestamp_tolerance_is_error() {
        let toml = format!(
            r#"
[sources.incidentio]
signing_secret = "{}"
timestamp_tolerance_secs = 0
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio: timestamp_tolerance_secs must not be zero")))
        );
    }

    #[test]
    fn load_invalid_toml_returns_load_error() {
        let f = write_toml("this is not { valid toml");
        let result = load(Some(f.path()));
        assert!(matches!(result, Err(ConfigError::Load(_))));
    }
}
