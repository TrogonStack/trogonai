use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;

use crate::source::discord::config::DiscordBotToken;
use crate::source::github::config::GitHubWebhookSecret;
use crate::source::gitlab::config::GitLabWebhookSecret;
use crate::source::incidentio::config::IncidentioConfig as IncidentioSourceConfig;
use crate::source::incidentio::incidentio_signing_secret::IncidentioSigningSecret;
use crate::source::linear::config::LinearWebhookSecret;
use crate::source::microsoft_graph::MicrosoftGraphClientState;
use crate::source::notion::NotionVerificationToken;
use crate::source::sentry::SentryClientSecret;
use crate::source::slack::config::SlackSigningSecret;
use crate::source::telegram::config::{
    TelegramBotToken, TelegramPublicWebhookUrl, TelegramWebhookRegistrationConfig, TelegramWebhookSecret,
};
use crate::source::twitter::config::TwitterConsumerSecret;
use crate::source_integration_id::{SourceIntegrationId, SourceIntegrationIdError};
use confique::Config;
#[cfg(test)]
use trogon_nats::NatsAuth;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{NatsToken, SubjectTokenViolation};
use trogon_service_config::{NatsArgs, NatsConfigSection, load_config, resolve_nats};
use trogon_std::{NonZeroDuration, ZeroDuration};

use crate::source_status::SourceStatus;

const DEFAULT_STREAM_MAX_AGE_SECS: u64 = 604_800;
const DEFAULT_NATS_ACK_TIMEOUT_SECS: u64 = 10;
const DEFAULT_SLACK_TIMESTAMP_MAX_DRIFT_SECS: u64 = 300;
const DEFAULT_LINEAR_TIMESTAMP_TOLERANCE_SECS: u64 = 60;
const DEFAULT_INCIDENTIO_TIMESTAMP_TOLERANCE_SECS: u64 = 300;

#[derive(Debug)]
struct ZeroNotAllowed;

impl fmt::Display for ZeroNotAllowed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("must be greater than zero")
    }
}

impl std::error::Error for ZeroNotAllowed {}

#[derive(Debug)]
struct DurationTooLong {
    max_secs: u64,
}

impl DurationTooLong {
    fn new(max_secs: u64) -> Self {
        Self { max_secs }
    }
}

impl fmt::Display for DurationTooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.max_secs == 1 {
            f.write_str("must not exceed 1 second")
        } else {
            write!(f, "must not exceed {} seconds", self.max_secs)
        }
    }
}

impl std::error::Error for DurationTooLong {}

const SENTRY_MAX_ACK_TIMEOUT_SECS: u64 = 1;

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(untagged)]
enum SecretInput {
    Literal(String),
    Env { env: String },
}

impl SecretInput {
    fn resolve(self) -> Result<String, SecretInputError> {
        match self {
            Self::Literal(value) => Ok(value),
            Self::Env { env } => {
                let name = env.trim();
                if name.is_empty() {
                    return Err(SecretInputError::EmptyEnvName);
                }
                env::var(name).map_err(|error| match error {
                    env::VarError::NotPresent => SecretInputError::MissingEnv { name: name.to_string() },
                    env::VarError::NotUnicode(_) => SecretInputError::InvalidUnicodeEnv { name: name.to_string() },
                })
            }
        }
    }
}

#[derive(Debug)]
enum SecretInputError {
    EmptyEnvName,
    MissingEnv { name: String },
    InvalidUnicodeEnv { name: String },
}

impl fmt::Display for SecretInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyEnvName => f.write_str("env var name must not be empty"),
            Self::MissingEnv { name } => write!(f, "env var '{name}' is not set"),
            Self::InvalidUnicodeEnv { name } => write!(f, "env var '{name}' is not valid unicode"),
        }
    }
}

impl std::error::Error for SecretInputError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TelegramWebhookRegistrationMode {
    #[default]
    Manual,
    Startup,
}

impl TelegramWebhookRegistrationMode {
    fn registers_on_startup(self) -> bool {
        matches!(self, Self::Startup)
    }
}

impl std::str::FromStr for TelegramWebhookRegistrationMode {
    type Err = TelegramWebhookRegistrationModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Ok(Self::Manual),
            "startup" => Ok(Self::Startup),
            _ => Err(TelegramWebhookRegistrationModeError::new(value)),
        }
    }
}

#[derive(Debug)]
struct TelegramWebhookRegistrationModeError {
    value: String,
}

impl TelegramWebhookRegistrationModeError {
    fn new(value: impl Into<String>) -> Self {
        Self { value: value.into() }
    }
}

impl fmt::Display for TelegramWebhookRegistrationModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unsupported registration mode '{}' ; expected 'manual' or 'startup'",
            self.value
        )
    }
}

impl std::error::Error for TelegramWebhookRegistrationModeError {}

#[derive(Debug)]
pub enum ConfigValidationError {
    InvalidIntegrationId {
        source: &'static str,
        integration: String,
        error: SourceIntegrationIdError,
    },
    InvalidField {
        source: &'static str,
        field: &'static str,
        error: Box<dyn std::error::Error + 'static>,
    },
    InvalidIntegrationField {
        source: &'static str,
        integration: String,
        field: &'static str,
        error: Box<dyn std::error::Error + 'static>,
    },
    MissingIntegrationField {
        source: &'static str,
        integration: String,
        field: &'static str,
    },
    InvalidSubjectToken {
        source: &'static str,
        field: &'static str,
        violation: SubjectTokenViolation,
    },
    InvalidIntegrationSubjectToken {
        source: &'static str,
        integration: String,
        field: &'static str,
        violation: SubjectTokenViolation,
    },
}

impl ConfigValidationError {
    fn invalid_integration_id(
        source: &'static str,
        integration: impl Into<String>,
        error: SourceIntegrationIdError,
    ) -> Self {
        Self::InvalidIntegrationId {
            source,
            integration: integration.into(),
            error,
        }
    }

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

    fn invalid_integration<E>(
        source: &'static str,
        integration: &SourceIntegrationId,
        field: &'static str,
        error: E,
    ) -> Self
    where
        E: std::error::Error + 'static,
    {
        Self::InvalidIntegrationField {
            source,
            integration: integration.as_str().to_string(),
            field,
            error: Box::new(error),
        }
    }

    fn missing_integration(source: &'static str, integration: &SourceIntegrationId, field: &'static str) -> Self {
        Self::MissingIntegrationField {
            source,
            integration: integration.as_str().to_string(),
            field,
        }
    }

    fn invalid_subject_token(source: &'static str, field: &'static str, violation: SubjectTokenViolation) -> Self {
        Self::InvalidSubjectToken {
            source,
            field,
            violation,
        }
    }

    fn invalid_integration_subject_token(
        source: &'static str,
        integration: &SourceIntegrationId,
        field: &'static str,
        violation: SubjectTokenViolation,
    ) -> Self {
        Self::InvalidIntegrationSubjectToken {
            source,
            integration: integration.as_str().to_string(),
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
            Self::InvalidIntegrationId {
                source,
                integration,
                error,
            } => write!(f, "{source}/{integration}: invalid integration id: {error}"),
            Self::InvalidField { source, field, error } => {
                if error.downcast_ref::<ZeroDuration>().is_some() {
                    write!(f, "{source}: {field} must not be zero")
                } else {
                    write!(f, "{source}: invalid {field}: {error}")
                }
            }
            Self::InvalidIntegrationField {
                source,
                integration,
                field,
                error,
            } => {
                if error.downcast_ref::<ZeroDuration>().is_some() {
                    write!(f, "{source}/{integration}: {field} must not be zero")
                } else {
                    write!(f, "{source}/{integration}: invalid {field}: {error}")
                }
            }
            Self::MissingIntegrationField {
                source,
                integration,
                field,
            } => write!(f, "{source}/{integration}: missing {field}"),
            Self::InvalidSubjectToken {
                source,
                field,
                violation,
            } => write!(f, "{source}: invalid {field}: {violation:?}"),
            Self::InvalidIntegrationSubjectToken {
                source,
                integration,
                field,
                violation,
            } => write!(f, "{source}/{integration}: invalid {field}: {violation:?}"),
        }
    }
}

impl std::error::Error for ConfigValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidIntegrationId { error, .. } => Some(error),
            Self::InvalidField { error, .. } => Some(error.as_ref()),
            Self::InvalidIntegrationField { error, .. } => Some(error.as_ref()),
            Self::MissingIntegrationField { .. } => None,
            Self::InvalidSubjectToken { .. } => None,
            Self::InvalidIntegrationSubjectToken { .. } => None,
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
    // TODO: restore dynamic server-info negotiation once the async-nats race is resolved.
    // See the TODO in main.rs for the full explanation.
    #[config(env = "TROGON_GATEWAY_NATS_MAX_PAYLOAD_BYTES", default = 1_048_576)]
    nats_max_payload_bytes: usize,
    #[config(nested)]
    sources: SourcesConfig,
}

#[derive(Config)]
struct HttpServerConfig {
    #[config(env = "TROGON_GATEWAY_PORT", default = 8080)]
    port: u16,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
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
    twitter: TwitterConfig,
    #[config(nested)]
    gitlab: GitlabConfig,
    #[config(nested)]
    incidentio: IncidentioConfig,
    #[config(nested)]
    linear: LinearConfig,
    #[config(nested)]
    microsoft_graph: MicrosoftGraphConfigInput,
    #[config(nested)]
    notion: NotionConfig,
    #[config(nested)]
    sentry: SentryConfig,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct GithubConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<GithubWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct DiscordConfig {
    status: Option<String>,
    bot_token: Option<SecretInput>,
    gateway_intents: Option<String>,
    #[config(default = "discord")]
    subject_prefix: String,
    #[config(default = "DISCORD")]
    stream_name: String,
    #[config(default = 604_800)]
    stream_max_age_secs: u64,
    #[config(default = 10)]
    nats_ack_timeout_secs: u64,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct SlackConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<SlackWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct TelegramConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<TelegramWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct TwitterConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<TwitterWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct GitlabConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<GitlabWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct LinearConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<LinearWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct MicrosoftGraphConfigInput {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<MicrosoftGraphWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct IncidentioConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<IncidentioWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct NotionConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<NotionWebhookConfig>>,
}

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct SentryConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<SentryWebhookConfig>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceIntegrationInput<T> {
    status: Option<String>,
    subject_prefix: Option<String>,
    stream_name: Option<String>,
    stream_max_age_secs: Option<u64>,
    nats_ack_timeout_secs: Option<u64>,
    webhook: Option<T>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GithubWebhookConfig {
    webhook_secret: Option<SecretInput>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SlackWebhookConfig {
    signing_secret: Option<SecretInput>,
    timestamp_max_drift_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TelegramWebhookConfig {
    webhook_secret: Option<SecretInput>,
    webhook_registration_mode: Option<String>,
    bot_token: Option<SecretInput>,
    public_webhook_url: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TwitterWebhookConfig {
    consumer_secret: Option<SecretInput>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GitlabWebhookConfig {
    webhook_secret: Option<SecretInput>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LinearWebhookConfig {
    webhook_secret: Option<SecretInput>,
    timestamp_tolerance_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MicrosoftGraphWebhookConfig {
    client_state: Option<SecretInput>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentioWebhookConfig {
    signing_secret: Option<SecretInput>,
    timestamp_tolerance_secs: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NotionWebhookConfig {
    verification_token: Option<SecretInput>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SentryWebhookConfig {
    client_secret: Option<SecretInput>,
}

pub struct ResolvedHttpServerConfig {
    pub port: u16,
}

pub struct SourceIntegration<T> {
    pub id: SourceIntegrationId,
    pub config: T,
}

impl<T> SourceIntegration<T> {
    fn new(id: SourceIntegrationId, config: T) -> Self {
        Self { id, config }
    }
}

pub struct ResolvedConfig {
    pub http_server: ResolvedHttpServerConfig,
    pub nats: trogon_nats::NatsConfig,
    pub nats_max_payload_bytes: NonZeroUsize,
    pub github: Vec<SourceIntegration<crate::source::github::GithubConfig>>,
    pub discord: Option<crate::source::discord::DiscordConfig>,
    pub slack: Vec<SourceIntegration<crate::source::slack::SlackConfig>>,
    pub telegram: Vec<SourceIntegration<crate::source::telegram::TelegramSourceConfig>>,
    pub twitter: Vec<SourceIntegration<crate::source::twitter::TwitterConfig>>,
    pub gitlab: Vec<SourceIntegration<crate::source::gitlab::GitlabConfig>>,
    pub incidentio: Vec<SourceIntegration<crate::source::incidentio::IncidentioConfig>>,
    pub linear: Vec<SourceIntegration<crate::source::linear::LinearConfig>>,
    pub microsoft_graph: Vec<SourceIntegration<crate::source::microsoft_graph::MicrosoftGraphConfig>>,
    pub notion: Vec<SourceIntegration<crate::source::notion::NotionConfig>>,
    pub sentry: Vec<SourceIntegration<crate::source::sentry::SentryConfig>>,
}

impl ResolvedConfig {
    pub fn has_any_source(&self) -> bool {
        !self.github.is_empty()
            || self.discord.is_some()
            || !self.slack.is_empty()
            || !self.telegram.is_empty()
            || !self.twitter.is_empty()
            || !self.gitlab.is_empty()
            || !self.incidentio.is_empty()
            || !self.linear.is_empty()
            || !self.microsoft_graph.is_empty()
            || !self.notion.is_empty()
            || !self.sentry.is_empty()
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

    let github = resolve_github_integrations(cfg.sources.github, &mut errors);
    let discord = resolve_discord(cfg.sources.discord, &mut errors);
    let slack = resolve_slack_integrations(cfg.sources.slack, &mut errors);
    let telegram = resolve_telegram_integrations(cfg.sources.telegram, &mut errors);
    let twitter = resolve_twitter_integrations(cfg.sources.twitter, &mut errors);
    let gitlab = resolve_gitlab_integrations(cfg.sources.gitlab, &mut errors);
    let incidentio = resolve_incidentio_integrations(cfg.sources.incidentio, &mut errors);
    let linear = resolve_linear_integrations(cfg.sources.linear, &mut errors);
    let microsoft_graph = resolve_microsoft_graph_integrations(cfg.sources.microsoft_graph, &mut errors);
    let notion = resolve_notion_integrations(cfg.sources.notion, &mut errors);
    let sentry = resolve_sentry_integrations(cfg.sources.sentry, &mut errors);

    let nats_max_payload_bytes = match NonZeroUsize::new(cfg.nats_max_payload_bytes) {
        Some(v) => v,
        None => {
            errors.push(ConfigValidationError::invalid(
                "nats",
                "nats_max_payload_bytes",
                ZeroNotAllowed,
            ));
            return Err(ConfigError::Validation(errors));
        }
    };

    if !errors.is_empty() {
        return Err(ConfigError::Validation(errors));
    }

    Ok(ResolvedConfig {
        http_server: ResolvedHttpServerConfig {
            port: cfg.http_server.port,
        },
        nats,
        nats_max_payload_bytes,
        github,
        discord,
        slack,
        telegram,
        twitter,
        gitlab,
        incidentio,
        linear,
        microsoft_graph,
        notion,
        sentry,
    })
}

fn resolve_github_integrations(
    section: GithubConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::github::GithubConfig>> {
    let GithubConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("github", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("github", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("github", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) = require_integration_value("github", &id, "webhook_secret", webhook.webhook_secret, errors)
        else {
            continue;
        };
        let webhook_secret = match GitHubWebhookSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "github",
                    &id,
                    "webhook_secret",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "github",
                id: &id,
                subject_source_prefix: "github",
                stream_source_prefix: "GITHUB",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::github::GithubConfig {
                webhook_secret,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_discord(
    section: DiscordConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<crate::source::discord::DiscordConfig> {
    let DiscordConfig {
        status,
        bot_token,
        gateway_intents,
        subject_prefix,
        stream_name,
        stream_max_age_secs,
        nats_ack_timeout_secs,
    } = section;

    if !resolve_source_status("discord", status.as_deref(), errors) {
        return None;
    }

    let token_str = match bot_token {
        Some(input) => match input.resolve() {
            Ok(value) => value,
            Err(error) => {
                errors.push(ConfigValidationError::invalid("discord", "bot_token", error));
                return None;
            }
        },
        None => return None,
    };
    let bot_token = match DiscordBotToken::new(token_str) {
        Ok(s) => s,
        Err(e) => {
            errors.push(ConfigValidationError::invalid("discord", "bot_token", e));
            return None;
        }
    };

    let intents = if let Some(s) = gateway_intents.as_deref().filter(|s| !s.is_empty()) {
        match crate::source::discord::config::parse_gateway_intents(s) {
            Ok(i) => i,
            Err(e) => {
                errors.push(ConfigValidationError::invalid("discord", "gateway_intents", e));
                return None;
            }
        }
    } else {
        crate::source::discord::config::default_intents()
    };

    let subject_prefix = match NatsToken::new(subject_prefix) {
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

    let stream_name = match NatsToken::new(stream_name) {
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

    let nats_ack_timeout = match NonZeroDuration::from_secs(nats_ack_timeout_secs) {
        Ok(d) => d,
        Err(err) => {
            errors.push(ConfigValidationError::invalid("discord", "nats_ack_timeout_secs", err));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(stream_max_age_secs) {
        Ok(age) => age,
        Err(err) => {
            errors.push(ConfigValidationError::invalid("discord", "stream_max_age_secs", err));
            return None;
        }
    };

    Some(crate::source::discord::DiscordConfig {
        bot_token,
        intents,
        subject_prefix,
        stream_name,
        stream_max_age,
        nats_ack_timeout,
    })
}

fn resolve_slack_integrations(
    section: SlackConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::slack::SlackConfig>> {
    let SlackConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("slack", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("slack", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("slack", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) = require_integration_value("slack", &id, "signing_secret", webhook.signing_secret, errors)
        else {
            continue;
        };
        let signing_secret = match SlackSigningSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "slack",
                    &id,
                    "signing_secret",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "slack",
                id: &id,
                subject_source_prefix: "slack",
                stream_source_prefix: "SLACK",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        let timestamp_max_drift = match NonZeroDuration::from_secs(
            webhook
                .timestamp_max_drift_secs
                .unwrap_or(DEFAULT_SLACK_TIMESTAMP_MAX_DRIFT_SECS),
        ) {
            Ok(duration) => duration,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "slack",
                    &id,
                    "timestamp_max_drift_secs",
                    error,
                ));
                continue;
            }
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::slack::SlackConfig {
                signing_secret,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
                timestamp_max_drift,
            },
        ));
    }
    integrations
}

fn resolve_telegram_integrations(
    section: TelegramConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::telegram::TelegramSourceConfig>> {
    let TelegramConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("telegram", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("telegram", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("telegram", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) = require_integration_value("telegram", &id, "webhook_secret", webhook.webhook_secret, errors)
        else {
            continue;
        };
        let webhook_secret = match TelegramWebhookSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "telegram",
                    &id,
                    "webhook_secret",
                    error,
                ));
                continue;
            }
        };
        let registration = match resolve_telegram_integration_registration(
            &id,
            webhook.webhook_registration_mode.as_deref().unwrap_or("manual"),
            webhook.bot_token,
            webhook.public_webhook_url,
            errors,
        ) {
            Some(Ok(registration)) => Some(registration),
            Some(Err(())) => continue,
            None => None,
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "telegram",
                id: &id,
                subject_source_prefix: "telegram",
                stream_source_prefix: "TELEGRAM",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::telegram::TelegramSourceConfig {
                webhook_secret,
                registration,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_telegram_integration_registration(
    id: &SourceIntegrationId,
    webhook_registration_mode: &str,
    bot_token: Option<SecretInput>,
    public_webhook_url: Option<String>,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<Result<TelegramWebhookRegistrationConfig, ()>> {
    let mode = match webhook_registration_mode.parse::<TelegramWebhookRegistrationMode>() {
        Ok(mode) => mode,
        Err(error) => {
            errors.push(ConfigValidationError::invalid_integration(
                "telegram",
                id,
                "webhook_registration_mode",
                error,
            ));
            return Some(Err(()));
        }
    };

    if !mode.registers_on_startup() {
        return None;
    }

    let public_webhook_url = public_webhook_url.filter(|value| !value.is_empty());
    let bot_token = match bot_token {
        Some(input) => match input.resolve() {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "telegram",
                    id,
                    "bot_token",
                    error,
                ));
                return Some(Err(()));
            }
        },
        None => None,
    };

    match (bot_token, public_webhook_url) {
        (None, None) => {
            errors.push(ConfigValidationError::missing_integration("telegram", id, "bot_token"));
            errors.push(ConfigValidationError::missing_integration(
                "telegram",
                id,
                "public_webhook_url",
            ));
            Some(Err(()))
        }
        (Some(_), None) => {
            errors.push(ConfigValidationError::missing_integration(
                "telegram",
                id,
                "public_webhook_url",
            ));
            Some(Err(()))
        }
        (None, Some(_)) => {
            errors.push(ConfigValidationError::missing_integration("telegram", id, "bot_token"));
            Some(Err(()))
        }
        (Some(bot_token), Some(public_webhook_url)) => {
            let bot_token = match TelegramBotToken::new(bot_token) {
                Ok(value) => value,
                Err(error) => {
                    errors.push(ConfigValidationError::invalid_integration(
                        "telegram",
                        id,
                        "bot_token",
                        error,
                    ));
                    return Some(Err(()));
                }
            };

            let public_webhook_url = match TelegramPublicWebhookUrl::new(public_webhook_url) {
                Ok(value) => value,
                Err(error) => {
                    errors.push(ConfigValidationError::invalid_integration(
                        "telegram",
                        id,
                        "public_webhook_url",
                        error,
                    ));
                    return Some(Err(()));
                }
            };

            Some(Ok(TelegramWebhookRegistrationConfig {
                bot_token,
                public_webhook_url,
            }))
        }
    }
}

fn resolve_twitter_integrations(
    section: TwitterConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::twitter::TwitterConfig>> {
    let TwitterConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("twitter", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("twitter", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("twitter", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) =
            require_integration_value("twitter", &id, "consumer_secret", webhook.consumer_secret, errors)
        else {
            continue;
        };
        let consumer_secret = match TwitterConsumerSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "twitter",
                    &id,
                    "consumer_secret",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "twitter",
                id: &id,
                subject_source_prefix: "twitter",
                stream_source_prefix: "TWITTER",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::twitter::TwitterConfig {
                consumer_secret,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_gitlab_integrations(
    section: GitlabConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::gitlab::GitlabConfig>> {
    let GitlabConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("gitlab", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("gitlab", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("gitlab", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) = require_integration_value("gitlab", &id, "webhook_secret", webhook.webhook_secret, errors)
        else {
            continue;
        };
        let webhook_secret = match GitLabWebhookSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "gitlab",
                    &id,
                    "webhook_secret",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "gitlab",
                id: &id,
                subject_source_prefix: "gitlab",
                stream_source_prefix: "GITLAB",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::gitlab::GitlabConfig {
                webhook_secret,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_linear_integrations(
    section: LinearConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::linear::LinearConfig>> {
    let LinearConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("linear", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("linear", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("linear", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) = require_integration_value("linear", &id, "webhook_secret", webhook.webhook_secret, errors)
        else {
            continue;
        };
        let webhook_secret = match LinearWebhookSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "linear",
                    &id,
                    "webhook_secret",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "linear",
                id: &id,
                subject_source_prefix: "linear",
                stream_source_prefix: "LINEAR",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::linear::LinearConfig {
                webhook_secret,
                subject_prefix,
                stream_name,
                stream_max_age,
                timestamp_tolerance: NonZeroDuration::from_secs(
                    webhook
                        .timestamp_tolerance_secs
                        .unwrap_or(DEFAULT_LINEAR_TIMESTAMP_TOLERANCE_SECS),
                )
                .ok(),
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_microsoft_graph_integrations(
    section: MicrosoftGraphConfigInput,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::microsoft_graph::MicrosoftGraphConfig>> {
    let MicrosoftGraphConfigInput {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("microsoft_graph", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("microsoft_graph", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("microsoft_graph", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(raw_client_state) =
            require_integration_value("microsoft_graph", &id, "client_state", webhook.client_state, errors)
        else {
            continue;
        };
        let client_state = match MicrosoftGraphClientState::new(raw_client_state) {
            Ok(client_state) => client_state,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "microsoft_graph",
                    &id,
                    "client_state",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "microsoft_graph",
                id: &id,
                subject_source_prefix: "microsoft-graph",
                stream_source_prefix: "MICROSOFT_GRAPH",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::microsoft_graph::MicrosoftGraphConfig {
                client_state,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_incidentio_integrations(
    section: IncidentioConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<IncidentioSourceConfig>> {
    let IncidentioConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("incidentio", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("incidentio", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("incidentio", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) =
            require_integration_value("incidentio", &id, "signing_secret", webhook.signing_secret, errors)
        else {
            continue;
        };
        let signing_secret = match IncidentioSigningSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "incidentio",
                    &id,
                    "signing_secret",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "incidentio",
                id: &id,
                subject_source_prefix: "incidentio",
                stream_source_prefix: "INCIDENTIO",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        let timestamp_tolerance = match NonZeroDuration::from_secs(
            webhook
                .timestamp_tolerance_secs
                .unwrap_or(DEFAULT_INCIDENTIO_TIMESTAMP_TOLERANCE_SECS),
        ) {
            Ok(duration) => duration,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "incidentio",
                    &id,
                    "timestamp_tolerance_secs",
                    error,
                ));
                continue;
            }
        };
        integrations.push(SourceIntegration::new(
            id,
            IncidentioSourceConfig {
                signing_secret,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
                timestamp_tolerance,
            },
        ));
    }
    integrations
}

fn resolve_notion_integrations(
    section: NotionConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::notion::NotionConfig>> {
    let NotionConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("notion", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("notion", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("notion", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(raw_token) =
            require_integration_value("notion", &id, "verification_token", webhook.verification_token, errors)
        else {
            continue;
        };
        let verification_token = match NotionVerificationToken::new(raw_token) {
            Ok(token) => token,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "notion",
                    &id,
                    "verification_token",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "notion",
                id: &id,
                subject_source_prefix: "notion",
                stream_source_prefix: "NOTION",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: DEFAULT_NATS_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::notion::NotionConfig {
                verification_token,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_sentry_integrations(
    section: SentryConfig,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::sentry::SentryConfig>> {
    let SentryConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("sentry", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("sentry", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("sentry", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(secret) = require_integration_value("sentry", &id, "client_secret", webhook.client_secret, errors)
        else {
            continue;
        };
        let client_secret = match SentryClientSecret::new(secret) {
            Ok(secret) => secret,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "sentry",
                    &id,
                    "client_secret",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "sentry",
                id: &id,
                subject_source_prefix: "sentry",
                stream_source_prefix: "SENTRY",
                subject_prefix: integration.subject_prefix,
                stream_name: integration.stream_name,
                stream_max_age_secs: integration.stream_max_age_secs,
                nats_ack_timeout_secs: integration.nats_ack_timeout_secs,
                default_nats_ack_timeout_secs: SENTRY_MAX_ACK_TIMEOUT_SECS,
            },
            errors,
        ) else {
            continue;
        };
        if integration.nats_ack_timeout_secs.unwrap_or(SENTRY_MAX_ACK_TIMEOUT_SECS) > SENTRY_MAX_ACK_TIMEOUT_SECS {
            errors.push(ConfigValidationError::invalid_integration(
                "sentry",
                &id,
                "nats_ack_timeout_secs",
                DurationTooLong::new(SENTRY_MAX_ACK_TIMEOUT_SECS),
            ));
            continue;
        }
        integrations.push(SourceIntegration::new(
            id,
            crate::source::sentry::SentryConfig {
                client_secret,
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
            },
        ));
    }
    integrations
}

fn resolve_integration_id(
    source: &'static str,
    raw_id: String,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<SourceIntegrationId> {
    match SourceIntegrationId::new(&raw_id) {
        Ok(id) => Some(id),
        Err(error) => {
            errors.push(ConfigValidationError::invalid_integration_id(source, raw_id, error));
            None
        }
    }
}

fn require_integration_value(
    source: &'static str,
    id: &SourceIntegrationId,
    field: &'static str,
    value: Option<SecretInput>,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<String> {
    match value {
        Some(value) => match value.resolve() {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(source, id, field, error));
                None
            }
        },
        None => {
            errors.push(ConfigValidationError::missing_integration(source, id, field));
            None
        }
    }
}

struct CommonIntegrationFieldsInput<'a> {
    source: &'static str,
    id: &'a SourceIntegrationId,
    subject_source_prefix: &'static str,
    stream_source_prefix: &'static str,
    subject_prefix: Option<String>,
    stream_name: Option<String>,
    stream_max_age_secs: Option<u64>,
    nats_ack_timeout_secs: Option<u64>,
    default_nats_ack_timeout_secs: u64,
}

fn resolve_common_integration_fields(
    input: CommonIntegrationFieldsInput<'_>,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<(NatsToken, NatsToken, StreamMaxAge, NonZeroDuration)> {
    let CommonIntegrationFieldsInput {
        source,
        id,
        subject_source_prefix,
        stream_source_prefix,
        subject_prefix,
        stream_name,
        stream_max_age_secs,
        nats_ack_timeout_secs,
        default_nats_ack_timeout_secs,
    } = input;

    let subject_prefix = match NatsToken::new(
        subject_prefix.unwrap_or_else(|| default_integration_subject_prefix(subject_source_prefix, id)),
    ) {
        Ok(token) => token,
        Err(error) => {
            errors.push(ConfigValidationError::invalid_integration_subject_token(
                source,
                id,
                "subject_prefix",
                error,
            ));
            return None;
        }
    };

    let stream_name = match NatsToken::new(
        stream_name.unwrap_or_else(|| default_integration_stream_name(stream_source_prefix, id)),
    ) {
        Ok(token) => token,
        Err(error) => {
            errors.push(ConfigValidationError::invalid_integration_subject_token(
                source,
                id,
                "stream_name",
                error,
            ));
            return None;
        }
    };

    let stream_max_age = match StreamMaxAge::from_secs(stream_max_age_secs.unwrap_or(DEFAULT_STREAM_MAX_AGE_SECS)) {
        Ok(age) => age,
        Err(error) => {
            errors.push(ConfigValidationError::invalid_integration(
                source,
                id,
                "stream_max_age_secs",
                error,
            ));
            return None;
        }
    };

    let nats_ack_timeout =
        match NonZeroDuration::from_secs(nats_ack_timeout_secs.unwrap_or(default_nats_ack_timeout_secs)) {
            Ok(duration) => duration,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    source,
                    id,
                    "nats_ack_timeout_secs",
                    error,
                ));
                return None;
            }
        };

    Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout))
}

fn default_integration_subject_prefix(source: &str, id: &SourceIntegrationId) -> String {
    format!("{}-{}", source, id.as_str())
}

fn default_integration_stream_name(source: &str, id: &SourceIntegrationId) -> String {
    format!("{}_{}", source, id.stream_name_suffix())
}

fn resolve_source_status(source: &'static str, status: Option<&str>, errors: &mut Vec<ConfigValidationError>) -> bool {
    let status = match status {
        Some(value) => match value.parse::<SourceStatus>() {
            Ok(status) => status,
            Err(err) => {
                errors.push(ConfigValidationError::invalid(source, "status", err));
                return false;
            }
        },
        None => SourceStatus::default(),
    };

    status.is_enabled()
}

fn resolve_integration_source_status(
    source: &'static str,
    id: &SourceIntegrationId,
    status: Option<&str>,
    errors: &mut Vec<ConfigValidationError>,
) -> bool {
    let status = match status {
        Some(value) => match value.parse::<SourceStatus>() {
            Ok(status) => status,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(source, id, "status", error));
                return false;
            }
        },
        None => SourceStatus::default(),
    };

    status.is_enabled()
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
        f.write_all(content.as_bytes()).expect("failed to write toml");
        f.flush().expect("failed to flush");
        f
    }

    fn minimal_toml() -> String {
        String::new()
    }

    fn github_toml(secret: &str) -> String {
        format!(
            r#"
[sources.github.integrations.primary.webhook]
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
[sources.slack.integrations.primary.webhook]
signing_secret = "{secret}"
"#
        )
    }

    fn telegram_toml(secret: &str) -> String {
        format!(
            r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "{secret}"
"#
        )
    }

    fn twitter_toml(secret: &str) -> String {
        format!(
            r#"
[sources.twitter.integrations.primary.webhook]
consumer_secret = "{secret}"
"#
        )
    }

    fn gitlab_toml(secret: &str) -> String {
        format!(
            r#"
[sources.gitlab.integrations.primary.webhook]
webhook_secret = "{secret}"
"#
        )
    }

    fn linear_toml(secret: &str) -> String {
        format!(
            r#"
[sources.linear.integrations.primary.webhook]
webhook_secret = "{secret}"
"#
        )
    }

    fn microsoft_graph_toml(client_state: &str) -> String {
        format!(
            r#"
[sources.microsoft_graph.integrations.primary.webhook]
client_state = "{client_state}"
"#
        )
    }

    fn incidentio_toml(secret: &str) -> String {
        format!(
            r#"
[sources.incidentio.integrations.primary.webhook]
signing_secret = "{secret}"
"#
        )
    }

    fn notion_toml(token: &str) -> String {
        format!(
            r#"
[sources.notion.integrations.primary.webhook]
verification_token = "{token}"
"#
        )
    }

    fn sentry_toml(secret: &str) -> String {
        format!(
            r#"
[sources.sentry.integrations.primary.webhook]
client_secret = "{secret}"
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
    fn github_disabled_returns_none() {
        let toml = r#"
[sources.github]
status = "disabled"

[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.github.is_empty());
        assert!(!cfg.has_any_source());
    }

    #[test]
    fn github_resolves_with_valid_secret() {
        let f = write_toml(&github_toml("my-gh-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.github.is_empty());
        assert!(cfg.has_any_source());
    }

    #[test]
    fn webhook_secret_can_resolve_from_env() {
        let toml = r#"
[sources.github.integrations.primary.webhook]
webhook_secret = { env = "PATH" }
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let github = cfg.github.first().expect("github should be configured");

        assert_eq!(github.config.webhook_secret.as_str(), env::var("PATH").unwrap());
    }

    #[test]
    fn webhook_secret_missing_env_is_invalid() {
        let toml = r#"
[sources.github.integrations.primary.webhook]
webhook_secret = { env = "TROGON_GATEWAY_TEST_WEBHOOK_SECRET_NOT_SET" }
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github/primary: invalid webhook_secret: env var 'TROGON_GATEWAY_TEST_WEBHOOK_SECRET_NOT_SET' is not set")))
        );
    }

    #[test]
    fn webhook_secret_empty_env_name_is_invalid() {
        let toml = r#"
[sources.github.integrations.primary.webhook]
webhook_secret = { env = "" }
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github/primary: invalid webhook_secret: env var name must not be empty")))
        );
    }

    #[test]
    fn github_webhook_integrations_resolve_with_integration_routing() {
        let toml = r#"
[sources.github.integrations.acme-main.webhook]
webhook_secret = "acme-secret"

[sources.github.integrations.other_org]
subject_prefix = "github-other"
stream_name = "GITHUB_OTHER"

[sources.github.integrations.other_org.webhook]
webhook_secret = "other-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");

        assert_eq!(cfg.github.len(), 2);
        assert_eq!(cfg.github[0].id.as_str(), "acme-main");
        assert_eq!(cfg.github[0].config.subject_prefix.as_str(), "github-acme-main");
        assert_eq!(cfg.github[0].config.stream_name.as_str(), "GITHUB_ACME-MAIN");
        assert_eq!(cfg.github[1].id.as_str(), "other_org");
        assert_eq!(cfg.github[1].config.subject_prefix.as_str(), "github-other");
        assert_eq!(cfg.github[1].config.stream_name.as_str(), "GITHUB_OTHER");
    }

    #[test]
    fn default_stream_names_preserve_hyphen_and_underscore_distinction() {
        let toml = r#"
[sources.github.integrations.acme-main.webhook]
webhook_secret = "hyphen-secret"

[sources.github.integrations.acme_main.webhook]
webhook_secret = "underscore-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");

        assert_eq!(cfg.github.len(), 2);
        assert_eq!(cfg.github[0].config.subject_prefix.as_str(), "github-acme-main");
        assert_eq!(cfg.github[0].config.stream_name.as_str(), "GITHUB_ACME-MAIN");
        assert_eq!(cfg.github[1].config.subject_prefix.as_str(), "github-acme_main");
        assert_eq!(cfg.github[1].config.stream_name.as_str(), "GITHUB_ACME_MAIN");
    }

    #[test]
    fn github_webhook_integration_missing_secret_is_invalid() {
        let toml = r#"
[sources.github.integrations.acme-main.webhook]
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github/acme-main: missing webhook_secret")))
        );
    }

    #[test]
    fn source_level_disabled_skips_webhook_integrations() {
        let toml = r#"
[sources.github]
status = "disabled"

[sources.github.integrations.acme-main.webhook]
webhook_secret = "acme-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");

        assert!(cfg.github.is_empty());
    }

    #[test]
    fn webhook_integration_ids_must_be_route_safe() {
        let toml = r#"
[sources.github.integrations."bad/id".webhook]
webhook_secret = "acme-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github/bad/id: invalid integration id")))
        );
    }

    #[test]
    fn legacy_webhook_tables_are_rejected() {
        let toml = r#"
[sources.github.webhooks.primary]
webhook_secret = "gh-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(matches!(result, Err(ConfigError::Load(_))));
    }

    #[test]
    fn integration_fields_are_rejected_in_webhook_blocks() {
        let toml = r#"
[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"
subject_prefix = "github-primary"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(matches!(result, Err(ConfigError::Load(_))));
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
    fn discord_disabled_returns_none() {
        let toml = r#"
[sources.discord]
status = "disabled"
bot_token = "Bot my-bot-token"
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
    fn discord_bot_token_missing_env_is_invalid() {
        let toml = r#"
[sources.discord]
bot_token = { env = "TROGON_GATEWAY_TEST_DISCORD_BOT_TOKEN_NOT_SET" }
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("discord: invalid bot_token: env var 'TROGON_GATEWAY_TEST_DISCORD_BOT_TOKEN_NOT_SET' is not set")))
        );
    }

    #[test]
    fn legacy_discord_env_var_fields_are_rejected() {
        let toml = r#"
[sources.discord]
TROGON_SOURCE_DISCORD_BOT_TOKEN = "Bot my-bot-token"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(matches!(result, Err(ConfigError::Load(_))));
    }

    #[test]
    fn slack_resolves_with_valid_secret() {
        let f = write_toml(&slack_toml("slack-signing-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.slack.is_empty());
    }

    #[test]
    fn slack_disabled_returns_none() {
        let toml = r#"
[sources.slack]
status = "disabled"

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.slack.is_empty());
    }

    #[test]
    fn telegram_resolves_with_valid_secret() {
        let f = write_toml(&telegram_toml("telegram-webhook-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.telegram.is_empty());
    }

    #[test]
    fn telegram_resolves_auto_registration_config() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "startup"
bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ"
public_webhook_url = "https://example.com/sources/telegram/primary/webhook"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let telegram = cfg.telegram.first().expect("telegram should be configured");
        let registration = telegram
            .config
            .registration
            .as_ref()
            .expect("registration should be configured");

        assert_eq!(registration.bot_token.as_str(), "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        assert_eq!(
            registration.public_webhook_url.as_str(),
            "https://example.com/sources/telegram/primary/webhook"
        );
    }

    #[test]
    fn telegram_startup_registration_without_public_webhook_url_is_invalid() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "startup"
bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: missing public_webhook_url")))
        );
    }

    #[test]
    fn telegram_startup_registration_without_credentials_is_invalid() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "startup"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: missing bot_token")) && errs.iter().any(|e| e.contains("telegram/primary: missing public_webhook_url")))
        );
    }

    #[test]
    fn telegram_startup_registration_with_empty_bot_token_is_invalid() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "startup"
bot_token = ""
public_webhook_url = "https://example.com/sources/telegram/primary/webhook"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: invalid bot_token")))
        );
    }

    #[test]
    fn telegram_startup_registration_without_bot_token_is_invalid() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "startup"
public_webhook_url = "https://example.com/sources/telegram/primary/webhook"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: missing bot_token")))
        );
    }

    #[test]
    fn telegram_http_public_webhook_url_is_invalid() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "startup"
bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ"
public_webhook_url = "http://example.com/sources/telegram/primary/webhook"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: invalid public_webhook_url")))
        );
    }

    #[test]
    fn telegram_registration_values_in_manual_mode_are_ignored() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "manual"
bot_token = "123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ"
public_webhook_url = "https://example.com/sources/telegram/primary/webhook"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let telegram = cfg.telegram.first().expect("telegram should be configured");

        assert!(telegram.config.registration.is_none());
    }

    #[test]
    fn telegram_invalid_webhook_registration_mode_is_invalid() {
        let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "sometimes"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));

        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: invalid webhook_registration_mode")))
        );
    }

    #[test]
    fn telegram_disabled_returns_none() {
        let toml = r#"
[sources.telegram]
status = "disabled"

[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.telegram.is_empty());
    }

    #[test]
    fn twitter_resolves_with_valid_consumer_secret() {
        let f = write_toml(&twitter_toml("twitter-consumer-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.twitter.is_empty());
    }

    #[test]
    fn twitter_disabled_returns_none() {
        let toml = r#"
[sources.twitter]
status = "disabled"

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.twitter.is_empty());
    }

    #[test]
    fn twitter_empty_consumer_secret_is_invalid() {
        let toml = r#"
[sources.twitter.integrations.primary.webhook]
consumer_secret = ""
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("twitter/primary: invalid consumer_secret")))
        );
    }

    #[test]
    fn gitlab_resolves_with_valid_secret() {
        let f = write_toml(&gitlab_toml("gitlab-webhook-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.gitlab.is_empty());
    }

    #[test]
    fn gitlab_disabled_returns_none() {
        let toml = r#"
[sources.gitlab]
status = "disabled"

[sources.gitlab.integrations.primary.webhook]
webhook_secret = "gitlab-webhook-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.gitlab.is_empty());
    }

    #[test]
    fn linear_resolves_with_valid_secret() {
        let f = write_toml(&linear_toml("linear-webhook-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.linear.is_empty());
    }

    #[test]
    fn linear_disabled_returns_none() {
        let toml = r#"
[sources.linear]
status = "disabled"

[sources.linear.integrations.primary.webhook]
webhook_secret = "linear-webhook-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.linear.is_empty());
    }

    #[test]
    fn microsoft_graph_resolves_with_valid_client_state() {
        let f = write_toml(&microsoft_graph_toml("microsoft-graph-client-state"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.microsoft_graph.is_empty());
    }

    #[test]
    fn microsoft_graph_disabled_returns_none() {
        let toml = r#"
[sources.microsoft_graph]
status = "disabled"

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.microsoft_graph.is_empty());
    }

    #[test]
    fn microsoft_graph_missing_client_state_returns_none_when_status_unspecified() {
        let toml = r#"
[sources.microsoft_graph]
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.microsoft_graph.is_empty());
    }

    #[test]
    fn microsoft_graph_enabled_without_client_state_is_invalid() {
        let toml = r#"
[sources.microsoft_graph.integrations.primary]
status = "enabled"

[sources.microsoft_graph.integrations.primary.webhook]

"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("microsoft_graph/primary: missing client_state")))
        );
    }

    #[test]
    fn incidentio_resolves_with_valid_secret() {
        let f = write_toml(&incidentio_toml(&incidentio_valid_test_secret()));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.incidentio.is_empty());
    }

    #[test]
    fn incidentio_disabled_returns_none() {
        let toml = format!(
            r#"
[sources.incidentio]
status = "disabled"

[sources.incidentio.integrations.primary.webhook]
signing_secret = "{}"
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.incidentio.is_empty());
    }

    #[test]
    fn notion_resolves_with_valid_token() {
        let f = write_toml(&notion_toml("notion-verification-token-example"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.notion.is_empty());
    }

    #[test]
    fn notion_disabled_returns_none() {
        let toml = r#"
[sources.notion]
status = "disabled"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.notion.is_empty());
    }

    #[test]
    fn sentry_resolves_with_valid_secret() {
        let f = write_toml(&sentry_toml("sentry-client-secret"));
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(!cfg.sentry.is_empty());
    }

    #[test]
    fn sentry_disabled_returns_none() {
        let toml = r#"
[sources.sentry]
status = "disabled"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.sentry.is_empty());
    }

    #[test]
    fn sentry_missing_client_secret_returns_none_when_status_unspecified() {
        let toml = r#"
[sources.sentry]
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.sentry.is_empty());
    }

    #[test]
    fn sentry_enabled_without_client_secret_is_invalid() {
        let toml = r#"
[sources.sentry.integrations.primary]
status = "enabled"

[sources.sentry.integrations.primary.webhook]

"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: missing client_secret")))
        );
    }

    #[test]
    fn sentry_enabled_with_surrounding_whitespace_without_client_secret_is_invalid() {
        let toml = r#"
[sources.sentry.integrations.primary]
status = " enabled "

[sources.sentry.integrations.primary.webhook]

"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: missing client_secret")))
        );
    }

    #[test]
    fn notion_missing_token_returns_none() {
        let toml = r#"
[sources.notion]
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        assert!(cfg.notion.is_empty());
    }

    #[test]
    fn linear_with_zero_timestamp_tolerance() {
        let toml = r#"
[sources.linear.integrations.primary.webhook]
webhook_secret = "linear-secret"
timestamp_tolerance_secs = 0
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let linear = cfg.linear.first().expect("linear should be configured");
        assert!(linear.config.timestamp_tolerance.is_none());
    }

    #[test]
    fn github_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.github.integrations.primary]
nats_ack_timeout_secs = 0

[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"
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
[sources.github.integrations.primary]
stream_max_age_secs = 0

[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"
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
[sources.slack.integrations.primary]
nats_ack_timeout_secs = 0

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
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
[sources.slack.integrations.primary.webhook]
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
[sources.slack.integrations.primary]
stream_max_age_secs = 0

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
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
[sources.telegram.integrations.primary]
nats_ack_timeout_secs = 0

[sources.telegram.integrations.primary.webhook]
webhook_secret = "tg-secret"
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
[sources.telegram.integrations.primary]
stream_max_age_secs = 0

[sources.telegram.integrations.primary.webhook]
webhook_secret = "tg-secret"
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
[sources.gitlab.integrations.primary]
nats_ack_timeout_secs = 0

[sources.gitlab.integrations.primary.webhook]
webhook_secret = "gl-secret"
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
[sources.gitlab.integrations.primary]
stream_max_age_secs = 0

[sources.gitlab.integrations.primary.webhook]
webhook_secret = "gl-secret"
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
[sources.linear.integrations.primary]
nats_ack_timeout_secs = 0

[sources.linear.integrations.primary.webhook]
webhook_secret = "lin-secret"
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
[sources.linear.integrations.primary]
stream_max_age_secs = 0

[sources.linear.integrations.primary.webhook]
webhook_secret = "lin-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn microsoft_graph_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.microsoft_graph.integrations.primary]
nats_ack_timeout_secs = 0

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("microsoft_graph/primary: nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn microsoft_graph_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.microsoft_graph.integrations.primary]
stream_max_age_secs = 0

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("microsoft_graph/primary: stream_max_age_secs must not be zero")))
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
[sources.github.integrations.primary]
subject_prefix = "has.dots"

[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"
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
[sources.github.integrations.primary]
stream_name = "has.dots"

[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"
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
            ConfigValidationError::invalid("discord", "stream_max_age_secs", ZeroDuration),
            ConfigValidationError::invalid_subject_token(
                "discord",
                "subject_prefix",
                SubjectTokenViolation::InvalidCharacter('.'),
            ),
        ]);
        let display = format!("{err}");
        assert!(display.contains("config validation errors:"));
        assert!(display.contains("discord: stream_max_age_secs must not be zero"));
        assert!(display.contains("discord: invalid subject_prefix: InvalidCharacter('.')"));
    }

    #[test]
    fn invalid_status_value_is_error() {
        let toml = r#"
[sources.github]
status = "maybe"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("invalid status")))
        );
    }

    #[test]
    fn config_validation_error_invalid_field_preserves_source() {
        let id = SourceIntegrationId::new("primary").unwrap();
        let err = ConfigValidationError::invalid_integration("incidentio", &id, "signing_secret", DummyConfigError);

        assert_eq!(
            err.to_string(),
            "incidentio/primary: invalid signing_secret: dummy config error"
        );
        assert!(err.source().is_some());
    }

    #[test]
    fn config_validation_error_invalid_subject_token_has_no_source() {
        let id = SourceIntegrationId::new("primary").unwrap();
        let err = ConfigValidationError::invalid_integration_subject_token(
            "incidentio",
            &id,
            "subject_prefix",
            SubjectTokenViolation::InvalidCharacter('.'),
        );

        assert_eq!(
            err.to_string(),
            "incidentio/primary: invalid subject_prefix: InvalidCharacter('.')"
        );
        assert!(err.source().is_none());
    }

    #[test]
    fn config_validation_error_missing_field_has_no_source() {
        let id = SourceIntegrationId::new("primary").unwrap();
        let err = ConfigValidationError::missing_integration("sentry", &id, "client_secret");

        assert_eq!(err.to_string(), "sentry/primary: missing client_secret");
        assert!(err.source().is_none());
    }

    #[test]
    fn duration_too_long_display_uses_plural_for_values_above_one_second() {
        let err = DurationTooLong::new(2);

        assert_eq!(err.to_string(), "must not exceed 2 seconds");
    }

    #[test]
    fn config_error_is_std_error() {
        let err = ConfigError::Validation(vec![ConfigValidationError::invalid(
            "discord",
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
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github/primary: invalid webhook_secret")))
        );
    }

    #[test]
    fn slack_empty_secret_is_invalid() {
        let f = write_toml(&slack_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid signing_secret")))
        );
    }

    #[test]
    fn telegram_empty_secret_is_invalid() {
        let f = write_toml(&telegram_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: invalid webhook_secret")))
        );
    }

    #[test]
    fn gitlab_empty_secret_is_invalid() {
        let f = write_toml(&gitlab_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("gitlab/primary: invalid webhook_secret")))
        );
    }

    #[test]
    fn linear_empty_secret_is_invalid() {
        let f = write_toml(&linear_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("linear/primary: invalid webhook_secret")))
        );
    }

    #[test]
    fn microsoft_graph_empty_client_state_is_invalid() {
        let f = write_toml(&microsoft_graph_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("microsoft_graph/primary: invalid client_state")))
        );
    }

    #[test]
    fn incidentio_empty_secret_is_invalid() {
        let f = write_toml(&incidentio_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: invalid signing_secret")))
        );
    }

    #[test]
    fn notion_empty_verification_token_is_invalid() {
        let f = write_toml(&notion_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion/primary: invalid verification_token")))
        );
    }

    #[test]
    fn sentry_empty_client_secret_is_invalid() {
        let f = write_toml(&sentry_toml(""));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: invalid client_secret")))
        );
    }

    #[test]
    fn incidentio_invalid_secret_is_invalid() {
        let f = write_toml(&incidentio_toml("whsec_not-base64!"));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: invalid signing_secret")))
        );
    }

    #[test]
    fn incidentio_secret_without_prefix_is_invalid() {
        let f = write_toml(&incidentio_toml("dGVzdC1zZWNyZXQ="));
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: invalid signing_secret")))
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
[sources.slack.integrations.primary]
subject_prefix = "has.dots"

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
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
[sources.telegram.integrations.primary]
subject_prefix = "has.dots"

[sources.telegram.integrations.primary.webhook]
webhook_secret = "tg-secret"
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
[sources.gitlab.integrations.primary]
subject_prefix = "has.dots"

[sources.gitlab.integrations.primary.webhook]
webhook_secret = "gl-secret"
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
[sources.linear.integrations.primary]
subject_prefix = "has.dots"

[sources.linear.integrations.primary.webhook]
webhook_secret = "lin-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("subject_prefix")))
        );
    }

    #[test]
    fn microsoft_graph_invalid_subject_prefix() {
        let toml = r#"
[sources.microsoft_graph.integrations.primary]
subject_prefix = "has.dots"

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("microsoft_graph/primary: invalid subject_prefix")))
        );
    }

    #[test]
    fn incidentio_invalid_subject_prefix() {
        let toml = format!(
            r#"
[sources.incidentio.integrations.primary]
subject_prefix = "has.dots"

[sources.incidentio.integrations.primary.webhook]
signing_secret = "{}"
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: invalid subject_prefix")))
        );
    }

    #[test]
    fn notion_invalid_subject_prefix() {
        let toml = r#"
[sources.notion.integrations.primary]
subject_prefix = "has.dots"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion/primary: invalid subject_prefix")))
        );
    }

    #[test]
    fn sentry_invalid_subject_prefix() {
        let toml = r#"
[sources.sentry.integrations.primary]
subject_prefix = "has.dots"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: invalid subject_prefix")))
        );
    }

    #[test]
    fn twitter_invalid_subject_prefix() {
        let toml = r#"
[sources.twitter.integrations.primary]
subject_prefix = "has.dots"

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("twitter/primary: invalid subject_prefix")))
        );
    }

    #[test]
    fn slack_invalid_stream_name() {
        let toml = r#"
[sources.slack.integrations.primary]
stream_name = "has.dots"

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
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
[sources.telegram.integrations.primary]
stream_name = "has.dots"

[sources.telegram.integrations.primary.webhook]
webhook_secret = "tg-secret"
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
[sources.gitlab.integrations.primary]
stream_name = "has.dots"

[sources.gitlab.integrations.primary.webhook]
webhook_secret = "gl-secret"
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
[sources.linear.integrations.primary]
stream_name = "has.dots"

[sources.linear.integrations.primary.webhook]
webhook_secret = "lin-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name")))
        );
    }

    #[test]
    fn microsoft_graph_invalid_stream_name() {
        let toml = r#"
[sources.microsoft_graph.integrations.primary]
stream_name = "has.dots"

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("microsoft_graph/primary: invalid stream_name")))
        );
    }

    #[test]
    fn incidentio_invalid_stream_name() {
        let toml = format!(
            r#"
[sources.incidentio.integrations.primary]
stream_name = "has.dots"

[sources.incidentio.integrations.primary.webhook]
signing_secret = "{}"
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: invalid stream_name")))
        );
    }

    #[test]
    fn notion_invalid_stream_name() {
        let toml = r#"
[sources.notion.integrations.primary]
stream_name = "has.dots"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion/primary: invalid stream_name")))
        );
    }

    #[test]
    fn sentry_invalid_stream_name() {
        let toml = r#"
[sources.sentry.integrations.primary]
stream_name = "has.dots"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: invalid stream_name")))
        );
    }

    #[test]
    fn twitter_invalid_stream_name() {
        let toml = r#"
[sources.twitter.integrations.primary]
stream_name = "has.dots"

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("twitter/primary: invalid stream_name")))
        );
    }

    #[test]
    fn incidentio_zero_nats_ack_timeout_is_error() {
        let toml = format!(
            r#"
[sources.incidentio.integrations.primary]
nats_ack_timeout_secs = 0

[sources.incidentio.integrations.primary.webhook]
signing_secret = "{}"
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn incidentio_zero_stream_max_age_is_error() {
        let toml = format!(
            r#"
[sources.incidentio.integrations.primary]
stream_max_age_secs = 0

[sources.incidentio.integrations.primary.webhook]
signing_secret = "{}"
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn notion_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.notion.integrations.primary]
nats_ack_timeout_secs = 0

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion/primary: nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn notion_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.notion.integrations.primary]
stream_max_age_secs = 0

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion/primary: stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn sentry_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.sentry.integrations.primary]
nats_ack_timeout_secs = 0

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn twitter_zero_nats_ack_timeout_is_error() {
        let toml = r#"
[sources.twitter.integrations.primary]
nats_ack_timeout_secs = 0

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("twitter/primary: nats_ack_timeout_secs must not be zero")))
        );
    }

    #[test]
    fn sentry_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.sentry.integrations.primary]
stream_max_age_secs = 0

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn twitter_zero_stream_max_age_is_error() {
        let toml = r#"
[sources.twitter.integrations.primary]
stream_max_age_secs = 0

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("twitter/primary: stream_max_age_secs must not be zero")))
        );
    }

    #[test]
    fn sentry_nats_ack_timeout_over_one_second_is_error() {
        let toml = r#"
[sources.sentry.integrations.primary]
nats_ack_timeout_secs = 2

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: invalid nats_ack_timeout_secs: must not exceed 1 second")))
        );
    }

    #[test]
    fn incidentio_zero_timestamp_tolerance_is_error() {
        let toml = format!(
            r#"
[sources.incidentio.integrations.primary.webhook]
signing_secret = "{}"
timestamp_tolerance_secs = 0
"#,
            incidentio_valid_test_secret()
        );
        let f = write_toml(&toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: timestamp_tolerance_secs must not be zero")))
        );
    }

    #[test]
    fn load_invalid_toml_returns_load_error() {
        let f = write_toml("this is not { valid toml");
        let result = load(Some(f.path()));
        assert!(matches!(result, Err(ConfigError::Load(_))));
    }

    #[test]
    fn nats_max_payload_bytes_zero_is_error() {
        let toml = r#"
nats_max_payload_bytes = 0

[sources.linear.integrations.primary.webhook]
webhook_secret = "linear-secret"
"#;
        let f = write_toml(toml);
        let result = load(Some(f.path()));
        assert!(
            matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("nats_max_payload_bytes")))
        );
    }
}
