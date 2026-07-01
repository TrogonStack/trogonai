use std::collections::BTreeMap;
use std::path::Path;

use crate::source::datadog::DatadogWebhookToken;
use crate::source::discord::config::DiscordBotToken;
use crate::source::github::config::GitHubWebhookSecret;
use crate::source::gitlab::GitLabSigningToken;
use crate::source::incidentio::config::IncidentioConfig as IncidentioSourceConfig;
use crate::source::incidentio::incidentio_signing_secret::IncidentioSigningSecret;
use crate::source::linear::config::LinearWebhookSecret;
use crate::source::microsoft_graph::MicrosoftGraphClientState;
use crate::source::notion::NotionVerificationToken;
use crate::source::sentry::SentryClientSecret;
use crate::source::slack::config::{
    SlackAppToken, SlackSigningSecret, SlackSocketModeConfig as SlackSocketModeSourceConfig, SlackTransportConfig,
    SlackWebhookConfig as SlackWebhookSourceConfig,
};
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
use trogon_std::env::{ReadEnv, SystemEnv};
use trogon_std::{NonZeroDuration, ZeroDuration};

use crate::constants::{
    DEFAULT_GITLAB_TIMESTAMP_TOLERANCE_SECS, DEFAULT_INCIDENTIO_TIMESTAMP_TOLERANCE_SECS,
    DEFAULT_LINEAR_TIMESTAMP_TOLERANCE_SECS, DEFAULT_NATS_ACK_TIMEOUT_SECS, DEFAULT_SLACK_TIMESTAMP_MAX_DRIFT_SECS,
    DEFAULT_STREAM_MAX_AGE_SECS,
};
use crate::source_status::SourceStatus;

#[derive(Debug, thiserror::Error)]
enum DurationTooLong {
    #[error("must not exceed 1 second")]
    OneSecond,
    #[error("must not exceed {0} seconds")]
    Many(u64),
}

impl DurationTooLong {
    fn new(max_secs: u64) -> Self {
        if max_secs == 1 {
            Self::OneSecond
        } else {
            Self::Many(max_secs)
        }
    }
}

const SENTRY_MAX_ACK_TIMEOUT_SECS: u64 = 1;

#[derive(Debug, thiserror::Error)]
#[error("configure exactly one of webhook or socket_mode")]
struct SlackTransportConflict;

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(untagged)]
enum SecretInput {
    Literal(String),
    Env { env: String },
}

impl SecretInput {
    fn resolve(self, env: &impl ReadEnv) -> Result<String, SecretInputError> {
        match self {
            Self::Literal(value) => Ok(value),
            Self::Env { env: var_name } => {
                let name = var_name.trim();
                if name.is_empty() {
                    return Err(SecretInputError::EmptyEnvName);
                }
                env.var(name).map_err(|error| match error {
                    std::env::VarError::NotPresent => SecretInputError::MissingEnv { name: name.to_string() },
                    std::env::VarError::NotUnicode(_) => SecretInputError::InvalidUnicodeEnv { name: name.to_string() },
                })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum SecretInputError {
    #[error("env var name must not be empty")]
    EmptyEnvName,
    #[error("env var '{name}' is not set")]
    MissingEnv { name: String },
    #[error("env var '{name}' is not valid unicode")]
    InvalidUnicodeEnv { name: String },
}

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

#[derive(Debug, thiserror::Error)]
#[error("unsupported registration mode '{value}' ; expected 'manual' or 'startup'")]
struct TelegramWebhookRegistrationModeError {
    value: String,
}

impl TelegramWebhookRegistrationModeError {
    fn new(value: impl Into<String>) -> Self {
        Self { value: value.into() }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigValidationError {
    #[error("{source_name}/{integration}: invalid integration id: {error}")]
    InvalidIntegrationId {
        source_name: &'static str,
        integration: String,
        #[source]
        error: SourceIntegrationIdError,
    },
    #[error("{source_name}: {field} must not be zero")]
    ZeroField {
        source_name: &'static str,
        field: &'static str,
    },
    #[error("{source_name}/{integration}: {field} must not be zero")]
    ZeroIntegrationField {
        source_name: &'static str,
        integration: String,
        field: &'static str,
    },
    #[error("{source_name}: invalid {field}: {error}")]
    InvalidField {
        source_name: &'static str,
        field: &'static str,
        #[source]
        error: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("{source_name}/{integration}: invalid {field}: {error}")]
    InvalidIntegrationField {
        source_name: &'static str,
        integration: String,
        field: &'static str,
        #[source]
        error: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("{source_name}/{integration}: missing {field}")]
    MissingIntegrationField {
        source_name: &'static str,
        integration: String,
        field: &'static str,
    },
    #[error("{source_name}: invalid {field}: {violation:?}")]
    InvalidSubjectToken {
        source_name: &'static str,
        field: &'static str,
        violation: SubjectTokenViolation,
    },
    #[error("{source_name}/{integration}: invalid {field}: {violation:?}")]
    InvalidIntegrationSubjectToken {
        source_name: &'static str,
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
            source_name: source,
            integration: integration.into(),
            error,
        }
    }

    fn invalid<E>(source: &'static str, field: &'static str, error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        if std::any::TypeId::of::<E>() == std::any::TypeId::of::<ZeroDuration>() {
            return Self::ZeroField {
                source_name: source,
                field,
            };
        }

        Self::InvalidField {
            source_name: source,
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
        E: std::error::Error + Send + Sync + 'static,
    {
        if std::any::TypeId::of::<E>() == std::any::TypeId::of::<ZeroDuration>() {
            return Self::ZeroIntegrationField {
                source_name: source,
                integration: integration.as_str().to_string(),
                field,
            };
        }

        Self::InvalidIntegrationField {
            source_name: source,
            integration: integration.as_str().to_string(),
            field,
            error: Box::new(error),
        }
    }

    fn missing_integration(source: &'static str, integration: &SourceIntegrationId, field: &'static str) -> Self {
        Self::MissingIntegrationField {
            source_name: source,
            integration: integration.as_str().to_string(),
            field,
        }
    }

    fn invalid_subject_token(source: &'static str, field: &'static str, violation: SubjectTokenViolation) -> Self {
        Self::InvalidSubjectToken {
            source_name: source,
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
            source_name: source,
            integration: integration.as_str().to_string(),
            field,
            violation,
        }
    }

    #[cfg(test)]
    fn contains(&self, needle: &str) -> bool {
        format!("{self}").contains(needle)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{}", self.display_errors())]
pub(crate) struct ValidationErrors(Vec<ConfigValidationError>);

impl ValidationErrors {
    fn display_errors(&self) -> String {
        let mut out = String::from("config validation errors:\n");
        for error in &self.0 {
            out.push_str(&format!("  - {error}\n"));
        }
        out
    }
}

impl std::ops::Deref for ValidationErrors {
    type Target = [ConfigValidationError];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to load config: {0}")]
    Load(#[source] confique::Error),
    #[error(transparent)]
    Validation(ValidationErrors),
}

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
    #[config(nested)]
    datadog: DatadogConfig,
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
    integrations: BTreeMap<String, SlackIntegrationInput>,
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

#[derive(Config)]
#[config(layer_attr(serde(deny_unknown_fields)))]
struct DatadogConfig {
    status: Option<String>,
    #[config(default = {})]
    integrations: BTreeMap<String, SourceIntegrationInput<DatadogWebhookConfig>>,
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
struct SlackIntegrationInput {
    status: Option<String>,
    subject_prefix: Option<String>,
    stream_name: Option<String>,
    stream_max_age_secs: Option<u64>,
    nats_ack_timeout_secs: Option<u64>,
    webhook: Option<SlackWebhookConfig>,
    socket_mode: Option<SlackSocketModeConfig>,
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
struct SlackSocketModeConfig {
    app_token: Option<SecretInput>,
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
    signing_token: Option<SecretInput>,
    timestamp_tolerance_secs: Option<u64>,
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DatadogWebhookConfig {
    webhook_token: Option<SecretInput>,
    timestamp_tolerance_secs: Option<u64>,
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
    pub datadog: Vec<SourceIntegration<crate::source::datadog::DatadogConfig>>,
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
            || !self.datadog.is_empty()
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
    let env = SystemEnv;
    let mut errors = Vec::new();

    let github = resolve_github_integrations(cfg.sources.github, &env, &mut errors);
    let discord = resolve_discord(cfg.sources.discord, &env, &mut errors);
    let slack = resolve_slack_integrations(cfg.sources.slack, &env, &mut errors);
    let telegram = resolve_telegram_integrations(cfg.sources.telegram, &env, &mut errors);
    let twitter = resolve_twitter_integrations(cfg.sources.twitter, &env, &mut errors);
    let gitlab = resolve_gitlab_integrations(cfg.sources.gitlab, &env, &mut errors);
    let incidentio = resolve_incidentio_integrations(cfg.sources.incidentio, &env, &mut errors);
    let linear = resolve_linear_integrations(cfg.sources.linear, &env, &mut errors);
    let microsoft_graph = resolve_microsoft_graph_integrations(cfg.sources.microsoft_graph, &env, &mut errors);
    let notion = resolve_notion_integrations(cfg.sources.notion, &env, &mut errors);
    let sentry = resolve_sentry_integrations(cfg.sources.sentry, &env, &mut errors);
    let datadog = resolve_datadog_integrations(cfg.sources.datadog, &env, &mut errors);

    if !errors.is_empty() {
        return Err(ConfigError::Validation(ValidationErrors(errors)));
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
        twitter,
        gitlab,
        incidentio,
        linear,
        microsoft_graph,
        notion,
        sentry,
        datadog,
    })
}

fn resolve_github_integrations(
    section: GithubConfig,
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("github", &id, "webhook"));
            continue;
        };
        let Some(secret) =
            require_integration_value("github", &id, "webhook_secret", webhook.webhook_secret, env, errors)
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
    env: &impl ReadEnv,
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
        Some(input) => match input.resolve(env) {
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
    env: &impl ReadEnv,
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
        let transport = match resolve_slack_transport(&id, integration.webhook, integration.socket_mode, env, errors) {
            Some(transport) => transport,
            None => continue,
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::slack::SlackConfig {
                subject_prefix,
                stream_name,
                stream_max_age,
                nats_ack_timeout,
                transport,
            },
        ));
    }
    integrations
}

fn resolve_slack_transport(
    id: &SourceIntegrationId,
    webhook: Option<SlackWebhookConfig>,
    socket_mode: Option<SlackSocketModeConfig>,
    env: &impl ReadEnv,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<SlackTransportConfig> {
    match (webhook, socket_mode) {
        (Some(webhook), None) => resolve_slack_webhook_transport(id, webhook, env, errors),
        (None, Some(socket_mode)) => resolve_slack_socket_mode_transport(id, socket_mode, env, errors),
        (Some(_), Some(_)) => {
            errors.push(ConfigValidationError::invalid_integration(
                "slack",
                id,
                "transport",
                SlackTransportConflict,
            ));
            None
        }
        (None, None) => {
            errors.push(ConfigValidationError::missing_integration("slack", id, "transport"));
            None
        }
    }
}

fn resolve_slack_webhook_transport(
    id: &SourceIntegrationId,
    webhook: SlackWebhookConfig,
    env: &impl ReadEnv,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<SlackTransportConfig> {
    let secret = require_integration_value("slack", id, "signing_secret", webhook.signing_secret, env, errors)?;
    let signing_secret = match SlackSigningSecret::new(secret) {
        Ok(secret) => secret,
        Err(error) => {
            errors.push(ConfigValidationError::invalid_integration(
                "slack",
                id,
                "signing_secret",
                error,
            ));
            return None;
        }
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
                id,
                "timestamp_max_drift_secs",
                error,
            ));
            return None;
        }
    };

    Some(SlackTransportConfig::Webhook(SlackWebhookSourceConfig {
        signing_secret,
        timestamp_max_drift,
    }))
}

fn resolve_slack_socket_mode_transport(
    id: &SourceIntegrationId,
    socket_mode: SlackSocketModeConfig,
    env: &impl ReadEnv,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<SlackTransportConfig> {
    let token = require_integration_value("slack", id, "app_token", socket_mode.app_token, env, errors)?;
    let app_token = match SlackAppToken::new(token) {
        Ok(token) => token,
        Err(error) => {
            errors.push(ConfigValidationError::invalid_integration(
                "slack",
                id,
                "app_token",
                error,
            ));
            return None;
        }
    };

    Some(SlackTransportConfig::SocketMode(SlackSocketModeSourceConfig {
        app_token,
    }))
}

fn resolve_telegram_integrations(
    section: TelegramConfig,
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("telegram", &id, "webhook"));
            continue;
        };
        let Some(secret) =
            require_integration_value("telegram", &id, "webhook_secret", webhook.webhook_secret, env, errors)
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
            env,
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
    env: &impl ReadEnv,
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
        Some(input) => match input.resolve(env) {
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
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("twitter", &id, "webhook"));
            continue;
        };
        let Some(secret) =
            require_integration_value("twitter", &id, "consumer_secret", webhook.consumer_secret, env, errors)
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
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("gitlab", &id, "webhook"));
            continue;
        };
        let Some(token) = require_integration_value("gitlab", &id, "signing_token", webhook.signing_token, env, errors)
        else {
            continue;
        };
        let signing_token = match GitLabSigningToken::new(token) {
            Ok(token) => token,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "gitlab",
                    &id,
                    "signing_token",
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
        let timestamp_tolerance = match NonZeroDuration::from_secs(
            webhook
                .timestamp_tolerance_secs
                .unwrap_or(DEFAULT_GITLAB_TIMESTAMP_TOLERANCE_SECS),
        ) {
            Ok(duration) => duration,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "gitlab",
                    &id,
                    "timestamp_tolerance_secs",
                    error,
                ));
                continue;
            }
        };
        integrations.push(SourceIntegration::new(
            id,
            crate::source::gitlab::GitlabConfig {
                signing_token,
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

fn resolve_linear_integrations(
    section: LinearConfig,
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("linear", &id, "webhook"));
            continue;
        };
        let Some(secret) =
            require_integration_value("linear", &id, "webhook_secret", webhook.webhook_secret, env, errors)
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
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration(
                "microsoft_graph",
                &id,
                "webhook",
            ));
            continue;
        };
        let Some(raw_client_state) = require_integration_value(
            "microsoft_graph",
            &id,
            "client_state",
            webhook.client_state,
            env,
            errors,
        ) else {
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
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("incidentio", &id, "webhook"));
            continue;
        };
        let Some(secret) =
            require_integration_value("incidentio", &id, "signing_secret", webhook.signing_secret, env, errors)
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
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("notion", &id, "webhook"));
            continue;
        };
        let Some(raw_token) = require_integration_value(
            "notion",
            &id,
            "verification_token",
            webhook.verification_token,
            env,
            errors,
        ) else {
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
    env: &impl ReadEnv,
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
            errors.push(ConfigValidationError::missing_integration("sentry", &id, "webhook"));
            continue;
        };
        let Some(secret) =
            require_integration_value("sentry", &id, "client_secret", webhook.client_secret, env, errors)
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

fn resolve_datadog_integrations(
    section: DatadogConfig,
    env: &impl ReadEnv,
    errors: &mut Vec<ConfigValidationError>,
) -> Vec<SourceIntegration<crate::source::datadog::DatadogConfig>> {
    let DatadogConfig {
        status,
        integrations: configured_integrations,
    } = section;
    let mut integrations = Vec::new();
    if !resolve_source_status("datadog", status.as_deref(), errors) {
        return integrations;
    }
    for (raw_id, integration) in configured_integrations {
        let Some(id) = resolve_integration_id("datadog", raw_id, errors) else {
            continue;
        };
        if !resolve_integration_source_status("datadog", &id, integration.status.as_deref(), errors) {
            continue;
        }
        let Some(webhook) = integration.webhook else {
            continue;
        };
        let Some(token) =
            require_integration_value("datadog", &id, "webhook_token", webhook.webhook_token, env, errors)
        else {
            continue;
        };
        let webhook_token = match DatadogWebhookToken::new(token) {
            Ok(token) => token,
            Err(error) => {
                errors.push(ConfigValidationError::invalid_integration(
                    "datadog",
                    &id,
                    "webhook_token",
                    error,
                ));
                continue;
            }
        };
        let Some((subject_prefix, stream_name, stream_max_age, nats_ack_timeout)) = resolve_common_integration_fields(
            CommonIntegrationFieldsInput {
                source: "datadog",
                id: &id,
                subject_source_prefix: "datadog",
                stream_source_prefix: "DATADOG",
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
        let timestamp_tolerance = webhook
            .timestamp_tolerance_secs
            .and_then(|secs| NonZeroDuration::from_secs(secs).ok());
        integrations.push(SourceIntegration::new(
            id,
            crate::source::datadog::DatadogConfig {
                webhook_token,
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
    env: &impl ReadEnv,
    errors: &mut Vec<ConfigValidationError>,
) -> Option<String> {
    match value {
        Some(value) => match value.resolve(env) {
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
mod tests;
