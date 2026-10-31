use super::*;
use crate::source::google_pubsub::credentials::fixtures::FAKE_EXTERNAL_ACCOUNT;
use std::error::Error;
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

fn gitlab_signing_token() -> &'static str {
    "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
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

fn slack_socket_mode_toml(token: &str) -> String {
    format!(
        r#"
[sources.slack.integrations.primary.socket_mode]
intended_use = "development"
app_token = "{token}"
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

fn gitlab_toml(signing_token: &str) -> String {
    format!(
        r#"
[sources.gitlab.integrations.primary.webhook]
signing_token = "{signing_token}"
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

fn datadog_toml(token: &str) -> String {
    format!(
        r#"
[sources.datadog.integrations.primary.webhook]
webhook_token = "{token}"
"#
    )
}

fn plaid_toml(secret: &str) -> String {
    format!(
        r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "{secret}"
environment = "sandbox"
"#
    )
}

fn zoom_toml(token: &str) -> String {
    format!(
        r#"
[sources.zoom.integrations.primary.webhook]
secret_token = "{token}"
"#
    )
}

fn incidentio_valid_test_secret() -> String {
    ["whsec_", "dGVzdC1zZWNyZXQ="].concat()
}

#[derive(Debug, thiserror::Error)]
#[error("dummy config error")]
struct DummyConfigError;

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
webhook_secret = { env = "TROGON_GATEWAY_TEST_WEBHOOK_SECRET" }
"#;
    let f = write_toml(toml);
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("TROGON_GATEWAY_TEST_WEBHOOK_SECRET", "gh-secret-from-env");
    let gateway_config = load_config::<GatewayConfig>(Some(f.path())).expect("load failed");
    let cfg = resolve(gateway_config, &NatsArgs::default(), &env).expect("resolve failed");
    let github = cfg.github.first().expect("github should be configured");

    assert_eq!(github.config.webhook_secret.as_str(), "gh-secret-from-env");
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

fn test_scope() -> CredentialScope {
    static_config_integration_scope(SourceKind::GitHub, &SourceIntegrationId::new("primary").unwrap())
}

#[test]
fn secret_marker_with_base64_value_is_decoded() {
    let env = trogon_std::env::InMemoryEnv::new();
    let value =
        SecretInput::Literal("secret:gcp:service-account:base64:eyJ0eXBlIjoic2VydmljZV9hY2NvdW50In0=".to_string())
            .resolve(test_scope(), CredentialKind::WebhookSecret, &env)
            .expect("marked base64 secret should resolve");

    assert_eq!(value, "{\"type\":\"service_account\"}");
}

#[test]
fn secret_with_base64_prefix_is_decoded() {
    let env = trogon_std::env::InMemoryEnv::new();
    let value = SecretInput::Literal("base64:eyJsaW5lIjoiYVxuYiJ9Cm11bHRpCmxpbmUK".to_string())
        .resolve(test_scope(), CredentialKind::WebhookSecret, &env)
        .expect("base64 secret should resolve");

    assert_eq!(value, "{\"line\":\"a\\nb\"}\nmulti\nline\n");
}

#[test]
fn secret_with_invalid_base64_is_rejected_without_echoing_value() {
    let env = trogon_std::env::InMemoryEnv::new();
    let error = SecretInput::Literal("base64:not*base64".to_string())
        .resolve(test_scope(), CredentialKind::WebhookSecret, &env)
        .expect_err("invalid base64 should fail");

    assert!(matches!(
        error,
        SecretInputError::InvalidValue(ConfigValueError::InvalidBase64)
    ));
}

#[test]
fn webhook_secret_invalid_base64_is_reported_per_integration() {
    let toml = r#"
[sources.github.integrations.primary.webhook]
webhook_secret = "base64:%%%"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));

    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github/primary: invalid webhook_secret: value with 'base64:' prefix is not valid standard base64")))
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
    assert_eq!(
        discord.bot_token.as_ref().expect("bot token should be Some").as_str(),
        "Bot my-bot-token"
    );
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
fn discord_gateway_can_resolve_without_static_bot_token() {
    let toml = r#"
[sources.discord]
gateway_intents = "guilds,guild_messages"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    let discord = cfg.discord.as_ref().expect("discord should be Some");
    assert!(discord.bot_token.is_none());
}

#[test]
fn discord_empty_table_resolves_without_static_bot_token() {
    let toml = r#"
[sources.discord]
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    let discord = cfg.discord.as_ref().expect("discord should be Some");
    assert!(discord.bot_token.is_none());
    assert_eq!(discord.intents, crate::source::discord::config::default_intents());
    assert_eq!(discord.subject_prefix.as_str(), "discord");
    assert_eq!(discord.stream_name.as_str(), "DISCORD");
    assert_eq!(
        discord.stream_max_age,
        StreamMaxAge::from_secs(DEFAULT_STREAM_MAX_AGE_SECS).expect("non-zero")
    );
    assert_eq!(
        discord.nats_ack_timeout,
        NonZeroDuration::from_secs(DEFAULT_NATS_ACK_TIMEOUT_SECS).expect("non-zero")
    );
}

#[test]
fn discord_absent_table_returns_none() {
    let f = write_toml(&github_toml("gh-secret"));
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
    assert!(cfg.slack[0].config.webhook().is_some());
    assert!(cfg.slack[0].config.socket_mode().is_none());
}

fn slack_max_body_size(cfg: &ResolvedConfig) -> usize {
    cfg.slack[0].config.webhook().unwrap().max_body_size.as_usize()
}

#[test]
fn slack_body_limit_defaults_to_four_mebibytes() {
    let f = write_toml(&slack_toml("slack-signing-secret"));
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(slack_max_body_size(&cfg), 4 * 1024 * 1024);
}

#[test]
fn slack_body_limit_is_configurable_in_toml() {
    let toml = r#"
[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
max_body_bytes = 8388608
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(slack_max_body_size(&cfg), 8 * 1024 * 1024);
}

#[test]
fn slack_zero_body_limit_is_error() {
    let toml = r#"
[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
max_body_bytes = 0
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid max_body_bytes: must not be zero")))
    );
}

#[test]
fn slack_body_limit_from_unset_env_is_error() {
    let toml = r#"
[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
max_body_bytes = { env = "TROGON_GATEWAY_TEST_SLACK_MAX_BODY_BYTES_NOT_SET" }
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid max_body_bytes")))
    );
}

#[test]
fn byte_count_input_reads_a_whole_number_from_the_environment() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("SLACK_BODY", " 2048 ");
    let input = ByteCountInput::Env {
        env: "SLACK_BODY".to_string(),
    };
    assert_eq!(input.resolve(&env).unwrap().as_usize(), 2048);
}

#[test]
fn byte_count_input_decodes_an_encoded_environment_value() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("SLACK_BODY", "base64:MjA0OA==");
    let input = ByteCountInput::Env {
        env: "SLACK_BODY".to_string(),
    };
    assert_eq!(input.resolve(&env).unwrap().as_usize(), 2048);
}

#[test]
fn byte_count_input_rejects_an_invalid_encoded_environment_value() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("SLACK_BODY", "base64:!!!");
    let input = ByteCountInput::Env {
        env: "SLACK_BODY".to_string(),
    };
    assert!(matches!(input.resolve(&env), Err(ByteCountInputError::InvalidValue(_))));
}

#[test]
fn byte_count_input_rejects_text_and_zero_from_the_environment() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("SLACK_BODY", "4MiB");
    env.set("SLACK_BODY_ZERO", "0");
    let text = ByteCountInput::Env {
        env: "SLACK_BODY".to_string(),
    };
    let zero = ByteCountInput::Env {
        env: "SLACK_BODY_ZERO".to_string(),
    };
    assert!(matches!(
        text.resolve(&env),
        Err(ByteCountInputError::NotAWholeNumber { .. })
    ));
    assert!(matches!(zero.resolve(&env), Err(ByteCountInputError::Zero)));
}

fn slack_ack_budget(cfg: &ResolvedConfig) -> crate::source::slack::ack_budget::AckBudget {
    cfg.slack[0].config.ack_budget
}

#[test]
fn slack_ack_budget_defaults_to_the_slack_deadline_not_the_generic_timeout() {
    let f = write_toml(&slack_toml("slack-signing-secret"));
    let cfg = load(Some(f.path())).expect("load failed");
    let budget = slack_ack_budget(&cfg);
    assert_eq!(budget.limit(), std::time::Duration::from_millis(2800));
    assert_eq!(budget.capped_from(), None);
}

#[test]
fn slack_ack_timeout_within_the_deadline_is_kept() {
    let toml = r#"
[sources.slack.integrations.primary]
nats_ack_timeout_secs = 1

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    let budget = slack_ack_budget(&cfg);
    assert_eq!(budget.limit(), std::time::Duration::from_secs(1));
    assert_eq!(budget.capped_from(), None);
}

#[test]
fn slack_ack_timeout_beyond_the_deadline_is_capped_instead_of_rejected() {
    let toml = r#"
[sources.slack.integrations.primary]
nats_ack_timeout_secs = 10

[sources.slack.integrations.primary.socket_mode]
intended_use = "development"
app_token = "xapp-test-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    let budget = slack_ack_budget(&cfg);
    assert_eq!(budget.limit(), std::time::Duration::from_millis(2800));
    assert_eq!(
        budget.capped_from().map(std::time::Duration::from),
        Some(std::time::Duration::from_secs(10))
    );
}

#[test]
fn slack_socket_mode_resolves_with_valid_app_token() {
    let f = write_toml(&slack_socket_mode_toml("xapp-test-token"));
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(!cfg.slack.is_empty());
    assert!(cfg.slack[0].config.webhook().is_none());
    assert!(cfg.slack[0].config.socket_mode().is_some());
}

#[test]
fn slack_socket_mode_missing_app_token_is_invalid() {
    let toml = r#"
[sources.slack.integrations.primary.socket_mode]
intended_use = "development"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: missing app_token")))
    );
}

#[test]
fn slack_disabled_socket_mode_integration_is_skipped() {
    let toml = r#"
[sources.slack.integrations.primary]
status = "disabled"

[sources.slack.integrations.primary.socket_mode]
intended_use = "development"
app_token = "xapp-test-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.slack.is_empty());
}

#[test]
fn slack_socket_mode_defaults_to_the_public_slack_api() {
    let f = write_toml(&slack_socket_mode_toml("xapp-test-token"));
    let cfg = load(Some(f.path())).expect("load failed");
    let socket_mode = cfg.slack[0].config.socket_mode().unwrap();
    assert_eq!(
        socket_mode.api_base_url.connections_open_url().as_str(),
        "https://slack.com/api/apps.connections.open"
    );
}

#[test]
fn slack_socket_mode_accepts_a_custom_api_base_url() {
    let f = write_toml(
        r#"
[sources.slack.integrations.primary.socket_mode]
intended_use = "development"
app_token = "xapp-test-token"
api_base_url = "https://slack-gov.com/api/"
"#,
    );
    let cfg = load(Some(f.path())).expect("load failed");
    let socket_mode = cfg.slack[0].config.socket_mode().unwrap();
    assert_eq!(
        socket_mode.api_base_url.connections_open_url().as_str(),
        "https://slack-gov.com/api/apps.connections.open"
    );
}

#[test]
fn slack_socket_mode_rejects_an_invalid_api_base_url() {
    let f = write_toml(
        r#"
[sources.slack.integrations.primary.socket_mode]
intended_use = "development"
app_token = "xapp-test-token"
api_base_url = "ftp://slack-gov.com/api/"
"#,
    );
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid api_base_url: invalid Slack API base URL: must use http or https")))
    );
}

#[test]
fn slack_socket_mode_rejects_non_app_token() {
    let f = write_toml(&slack_socket_mode_toml("xoxb-bot-token"));
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid app_token: must start with xapp-")))
    );
}

#[test]
fn slack_socket_mode_requires_intended_use() {
    let f = write_toml(
        r#"
[sources.slack.integrations.primary.socket_mode]
app_token = "xapp-test-token"
"#,
    );
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid intended_use: required because Slack does not intend Socket Mode for widely distributed apps") && e.contains("'development' or 'behind_firewall'")))
    );
}

#[test]
fn slack_socket_mode_rejects_an_unknown_intended_use() {
    let f = write_toml(
        r#"
[sources.slack.integrations.primary.socket_mode]
app_token = "xapp-test-token"
intended_use = "production"
"#,
    );
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid intended_use: unsupported value 'production'; Slack does not intend Socket Mode for widely distributed apps, expected 'development' or 'behind_firewall'")))
    );
}

#[test]
fn slack_socket_mode_accepts_each_supported_intended_use() {
    for (value, expected) in [
        ("development", SlackSocketModeIntendedUse::Development),
        ("behind_firewall", SlackSocketModeIntendedUse::BehindFirewall),
    ] {
        let f = write_toml(&format!(
            r#"
[sources.slack.integrations.primary.socket_mode]
app_token = "xapp-test-token"
intended_use = "{value}"
"#
        ));
        let cfg = load(Some(f.path())).expect("load failed");
        assert_eq!(cfg.slack[0].config.socket_mode().unwrap().intended_use, expected);
    }
}

#[test]
fn slack_integration_rejects_webhook_and_socket_mode_together() {
    let toml = r#"
[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"

[sources.slack.integrations.primary.socket_mode]
intended_use = "development"
app_token = "xapp-test-token"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: invalid transport: configure exactly one of webhook or socket_mode")))
    );
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
fn gitlab_resolves_with_valid_signing_token() {
    let f = write_toml(&gitlab_toml(gitlab_signing_token()));
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(!cfg.gitlab.is_empty());
}

#[test]
fn gitlab_disabled_returns_none() {
    let toml = r#"
[sources.gitlab]
status = "disabled"

[sources.gitlab.integrations.primary.webhook]
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
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
fn datadog_resolves_with_valid_token() {
    let f = write_toml(&datadog_toml("datadog-webhook-token"));
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(!cfg.datadog.is_empty());
    assert!(cfg.has_any_source());
}

#[test]
fn datadog_disabled_is_empty() {
    let toml = r#"
[sources.datadog]
status = "disabled"

[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.datadog.is_empty());
}

#[test]
fn datadog_enabled_without_webhook_token_is_invalid() {
    let toml = r#"
[sources.datadog.integrations.primary]
status = "enabled"

[sources.datadog.integrations.primary.webhook]

"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("datadog/primary: missing webhook_token")))
    );
}

#[test]
fn datadog_empty_webhook_token_is_invalid() {
    let f = write_toml(&datadog_toml(""));
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("datadog/primary: invalid webhook_token")))
    );
}

#[test]
fn datadog_enabled_integration_without_webhook_block_is_invalid() {
    let toml = r#"
[sources.datadog.integrations.primary]
subject_prefix = "datadog-primary"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("datadog/primary: missing webhook")))
    );
}

#[test]
fn datadog_invalid_integration_id_is_invalid() {
    let toml = r#"
[sources.datadog.integrations."bad/id".webhook]
webhook_token = "datadog-webhook-token"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("datadog/bad/id: invalid integration id")))
    );
}

#[test]
fn datadog_integration_disabled_is_skipped() {
    let toml = r#"
[sources.datadog.integrations.primary]
status = "disabled"

[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.datadog.is_empty());
}

#[test]
fn datadog_zero_stream_max_age_is_error() {
    let toml = r#"
[sources.datadog.integrations.primary]
stream_max_age_secs = 0

[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("datadog/primary: stream_max_age_secs must not be zero")))
    );
}

#[test]
fn datadog_timestamp_tolerance_defaults_to_disabled() {
    let f = write_toml(&datadog_toml("datadog-webhook-token"));
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.datadog[0].config.timestamp_tolerance.is_none());
}

#[test]
fn datadog_timestamp_tolerance_resolves_when_set() {
    let toml = r#"
[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
timestamp_tolerance_secs = 300
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(
        cfg.datadog[0].config.timestamp_tolerance.map(std::time::Duration::from),
        Some(std::time::Duration::from_secs(300)),
    );
}

#[test]
fn datadog_zero_timestamp_tolerance_disables_check() {
    let toml = r#"
[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
timestamp_tolerance_secs = 0
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.datadog[0].config.timestamp_tolerance.is_none());
}

#[test]
fn datadog_webhook_token_header_defaults() {
    let f = write_toml(&datadog_toml("datadog-webhook-token"));
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(
        cfg.datadog[0].config.webhook_token_header.as_str(),
        "x-datadog-webhook-token",
    );
}

#[test]
fn datadog_custom_webhook_token_header_resolves() {
    let toml = r#"
[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
webhook_token_header = "X-Acme-Token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(cfg.datadog[0].config.webhook_token_header.as_str(), "x-acme-token");
}

#[test]
fn datadog_invalid_webhook_token_header_is_invalid() {
    let toml = r#"
[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
webhook_token_header = "bad header"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("datadog/primary: invalid webhook_token_header")))
    );
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
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
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
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
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
    assert!(matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name"))));
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
    assert!(matches!(
        cfg.nats.auth,
        NatsAuth::Token(trogon_nats::CredentialValue::Literal(ref token)) if token.as_str() == "mytoken"
    ));
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
            nats_token: Some("env://OVERRIDE_TOKEN".parse().expect("valid reference")),
            ..Default::default()
        },
    )
    .expect("load failed");

    assert_eq!(cfg.nats.servers, vec!["override:4222"]);
    assert!(matches!(
        cfg.nats.auth,
        NatsAuth::Token(trogon_nats::CredentialValue::Env(ref name)) if name == "OVERRIDE_TOKEN"
    ));
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
            nats_token: Some("env://OVERRIDE_TOKEN".parse().expect("valid reference")),
            ..Default::default()
        },
    )
    .expect("load failed");

    assert!(
        matches!(cfg.nats.auth, NatsAuth::Credentials(trogon_nats::Location::Path(ref path)) if path == std::path::Path::new("/path/to/override.creds"))
    );
}

#[test]
fn load_with_overrides_rejects_a_cli_user_sent_as_a_token() {
    let f = write_toml(&minimal_toml());
    let result = load_with_overrides(
        Some(f.path()),
        &NatsArgs {
            nats_user: Some("would-be-token".to_string()),
            ..Default::default()
        },
    );

    assert!(matches!(result, Err(ConfigError::Nats(NatsArgsError::UserSentAsToken))));
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
    let err = ConfigError::Validation(AggregateValidationError(vec![
        ConfigValidationError::invalid("discord", "stream_max_age_secs", ZeroDurationError),
        ConfigValidationError::invalid_subject_token(
            "discord",
            "subject_prefix",
            SubjectTokenViolationError::InvalidCharacter('.'),
        ),
    ]));
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
        SubjectTokenViolationError::InvalidCharacter('.'),
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
    let err = DurationTooLongError::new(2);

    assert_eq!(err.to_string(), "must not exceed 2 seconds");
}

#[test]
fn config_error_is_std_error() {
    let err = ConfigError::Validation(AggregateValidationError(vec![ConfigValidationError::invalid(
        "discord",
        "stream_max_age_secs",
        ZeroDurationError,
    )]));
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
fn gitlab_empty_signing_token_is_invalid() {
    let f = write_toml(&gitlab_toml(""));
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("gitlab/primary: invalid signing_token")))
    );
}

#[test]
fn gitlab_missing_signing_token_is_invalid() {
    let toml = r#"
[sources.gitlab.integrations.primary.webhook]
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("gitlab/primary: missing signing_token")))
    );
}

#[test]
fn gitlab_zero_timestamp_tolerance_is_error() {
    let toml = r#"
[sources.gitlab.integrations.primary.webhook]
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
timestamp_tolerance_secs = 0
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("gitlab/primary: timestamp_tolerance_secs must not be zero")))
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
    assert!(matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name"))));
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
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
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
    assert!(matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name"))));
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
    assert!(matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name"))));
}

#[test]
fn gitlab_invalid_stream_name() {
    let toml = r#"
[sources.gitlab.integrations.primary]
stream_name = "has.dots"

[sources.gitlab.integrations.primary.webhook]
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name"))));
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
    assert!(matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("stream_name"))));
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
fn github_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.github.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("github/primary: missing webhook")))
    );
}

#[test]
fn telegram_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.telegram.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("telegram/primary: missing webhook")))
    );
}

#[test]
fn twitter_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.twitter.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("twitter/primary: missing webhook")))
    );
}

#[test]
fn gitlab_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.gitlab.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("gitlab/primary: missing webhook")))
    );
}

#[test]
fn linear_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.linear.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("linear/primary: missing webhook")))
    );
}

#[test]
fn microsoft_graph_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.microsoft_graph.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("microsoft_graph/primary: missing webhook")))
    );
}

#[test]
fn incidentio_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.incidentio.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("incidentio/primary: missing webhook")))
    );
}

#[test]
fn notion_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.notion.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("notion/primary: missing webhook")))
    );
}

#[test]
fn sentry_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.sentry.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("sentry/primary: missing webhook")))
    );
}

#[test]
fn slack_enabled_integration_without_transport_is_invalid() {
    let toml = r#"
[sources.slack.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("slack/primary: missing transport")))
    );
}

#[test]
fn max_stream_max_age_is_none_without_sources() {
    let f = write_toml(&minimal_toml());
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(cfg.max_stream_max_age(), None);
}

#[test]
fn max_stream_max_age_picks_the_longest_across_sources() {
    let toml = r#"
[sources.github.integrations.primary]
stream_max_age_secs = 3600
[sources.github.integrations.primary.webhook]
webhook_secret = "short-lived"

[sources.slack.integrations.primary]
stream_max_age_secs = 7200
[sources.slack.integrations.primary.webhook]
signing_secret = "8f7e6d5c4b3a29180f1e2d3c4b5a69788f7e6d5c4b3a29180f1e2d3c4b5a6978"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");

    assert_eq!(
        cfg.max_stream_max_age(),
        Some(StreamMaxAge::from_secs(7200).expect("non-zero"))
    );
}

#[test]
fn longest_stream_max_age_is_none_when_empty() {
    assert_eq!(longest_stream_max_age(std::iter::empty()), None);
}

#[test]
fn longest_stream_max_age_keeps_the_larger_regardless_of_order() {
    let short = StreamMaxAge::from_secs(3600).expect("non-zero");
    let long = StreamMaxAge::from_secs(7200).expect("non-zero");

    assert_eq!(longest_stream_max_age([short, long].into_iter()), Some(long));
    assert_eq!(longest_stream_max_age([long, short].into_iter()), Some(long));
}

#[test]
fn longest_stream_max_age_lets_no_expiry_dominate_in_either_position() {
    let bounded = StreamMaxAge::from_secs(7200).expect("non-zero");

    assert_eq!(
        longest_stream_max_age([bounded, StreamMaxAge::NoExpiry].into_iter()),
        Some(StreamMaxAge::NoExpiry)
    );
    assert_eq!(
        longest_stream_max_age([StreamMaxAge::NoExpiry, bounded].into_iter()),
        Some(StreamMaxAge::NoExpiry)
    );
}

const PUBSUB_CREDENTIALS_FIELD: &str = r#"credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'"#;

fn google_pubsub_toml(subscription: &str) -> String {
    format!(
        r#"
[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "{subscription}"
{PUBSUB_CREDENTIALS_FIELD}
"#
    )
}

fn validation_errors(result: Result<ResolvedConfig, ConfigError>) -> AggregateValidationError {
    match result {
        Err(ConfigError::Validation(errors)) => errors,
        Err(other) => panic!("expected validation errors, got {other}"),
        Ok(_) => panic!("expected validation errors, config loaded"),
    }
}

#[test]
fn google_pubsub_resolves_with_defaults() {
    let f = write_toml(&google_pubsub_toml("projects/p/subscriptions/sub"));
    let cfg = load(Some(f.path())).expect("load failed");

    assert!(cfg.has_any_source());
    assert_eq!(cfg.google_pubsub.len(), 1);
    let integration = &cfg.google_pubsub[0];
    assert_eq!(integration.id.as_str(), "primary");
    assert_eq!(integration.config.subject_prefix.as_str(), "google-pubsub-primary");
    assert_eq!(integration.config.stream_name.as_str(), "GOOGLE_PUBSUB_PRIMARY");
    assert_eq!(
        std::time::Duration::from(integration.config.nats_ack_timeout).as_secs(),
        DEFAULT_NATS_ACK_TIMEOUT_SECS
    );
    let GooglePubSubTransportConfig::StreamingPull(streaming_pull) = &integration.config.transport;
    assert_eq!(streaming_pull.subscription.as_str(), "projects/p/subscriptions/sub");
    assert_eq!(streaming_pull.max_outstanding_messages, None);
}

#[test]
fn google_pubsub_resolves_explicit_fields() {
    let toml = r#"
[sources.google_pubsub.integrations.primary]
subject_prefix = "pubsub-events"
stream_name = "PUBSUB_EVENTS"
stream_max_age_secs = 7200
nats_ack_timeout_secs = 3

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
max_outstanding_messages = 50
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");

    let integration = &cfg.google_pubsub[0];
    assert_eq!(integration.config.subject_prefix.as_str(), "pubsub-events");
    assert_eq!(integration.config.stream_name.as_str(), "PUBSUB_EVENTS");
    assert_eq!(
        cfg.max_stream_max_age(),
        Some(StreamMaxAge::from_secs(7200).expect("non-zero"))
    );
    assert_eq!(
        std::time::Duration::from(integration.config.nats_ack_timeout).as_secs(),
        3
    );
    let GooglePubSubTransportConfig::StreamingPull(streaming_pull) = &integration.config.transport;
    assert_eq!(
        streaming_pull.max_outstanding_messages.map(MaxOutstandingMessages::get),
        Some(50)
    );
}

#[test]
fn google_pubsub_disabled_source_is_skipped() {
    let toml = r#"
[sources.google_pubsub]
status = "disabled"

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");

    assert!(cfg.google_pubsub.is_empty());
    assert!(!cfg.has_any_source());
}

#[test]
fn google_pubsub_disabled_integration_is_skipped() {
    let toml = r#"
[sources.google_pubsub.integrations.primary]
status = "disabled"

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");

    assert!(cfg.google_pubsub.is_empty());
}

#[test]
fn google_pubsub_invalid_integration_id_is_invalid() {
    let toml = r#"
[sources.google_pubsub.integrations."bad/id".streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/bad/id: invalid integration id"))
    );
}

#[test]
fn google_pubsub_without_transport_is_invalid() {
    let toml = r#"
[sources.google_pubsub.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: missing transport"))
    );
}

#[test]
fn google_pubsub_without_subscription_is_invalid() {
    let toml = r#"
[sources.google_pubsub.integrations.primary.streaming_pull]
max_outstanding_messages = 10
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: missing subscription"))
    );
}

#[test]
fn google_pubsub_malformed_subscription_is_invalid() {
    let f = write_toml(&google_pubsub_toml("my-subscription"));

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: invalid subscription"))
    );
}

#[test]
fn google_pubsub_zero_max_outstanding_messages_is_invalid() {
    let toml = r#"
[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
max_outstanding_messages = 0
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: invalid max_outstanding_messages"))
    );
}

#[test]
fn google_pubsub_zero_nats_ack_timeout_is_error() {
    let toml = r#"
[sources.google_pubsub.integrations.primary]
nats_ack_timeout_secs = 0

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: nats_ack_timeout_secs must not be zero"))
    );
}

#[test]
fn google_pubsub_zero_stream_max_age_is_error() {
    let toml = r#"
[sources.google_pubsub.integrations.primary]
stream_max_age_secs = 0

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: stream_max_age_secs must not be zero"))
    );
}

#[test]
fn google_pubsub_invalid_subject_prefix_is_error() {
    let toml = r#"
[sources.google_pubsub.integrations.primary]
subject_prefix = "bad.prefix"

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: invalid subject_prefix"))
    );
}

#[test]
fn google_pubsub_rejects_unknown_transport_keys() {
    let toml = r#"
[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
push_endpoint = "https://example.invalid"
"#;
    let f = write_toml(toml);

    assert!(matches!(load(Some(f.path())), Err(ConfigError::Load(_))));
}

#[test]
fn google_pubsub_rejects_unknown_integration_keys() {
    let toml = r#"
[sources.google_pubsub.integrations.primary]
push = true

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);

    assert!(matches!(load(Some(f.path())), Err(ConfigError::Load(_))));
}

fn google_pubsub_credentials_toml(credentials_line: &str) -> String {
    format!(
        r#"
[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
{credentials_line}
"#
    )
}

#[test]
fn google_pubsub_resolves_service_account_credentials() {
    let f = write_toml(&google_pubsub_toml("projects/p/subscriptions/sub"));
    let cfg = load(Some(f.path())).expect("load failed");

    let GooglePubSubTransportConfig::StreamingPull(streaming_pull) = &cfg.google_pubsub[0].config.transport;
    assert_eq!(
        streaming_pull.credentials.kind(),
        crate::source::google_pubsub::credentials::GoogleCredentialsKind::ServiceAccount
    );
}

#[test]
fn google_pubsub_resolves_external_account_credentials() {
    let f = write_toml(&google_pubsub_credentials_toml(&format!(
        "credentials = '{FAKE_EXTERNAL_ACCOUNT}'"
    )));
    let cfg = load(Some(f.path())).expect("load failed");

    let GooglePubSubTransportConfig::StreamingPull(streaming_pull) = &cfg.google_pubsub[0].config.transport;
    assert_eq!(
        streaming_pull.credentials.kind(),
        crate::source::google_pubsub::credentials::GoogleCredentialsKind::ExternalAccount
    );
}

#[test]
fn google_pubsub_credentials_debug_is_redacted() {
    let f = write_toml(&google_pubsub_toml("projects/p/subscriptions/sub"));
    let cfg = load(Some(f.path())).expect("load failed");

    let rendered = format!("{:?}", cfg.google_pubsub[0].config);
    assert!(rendered.contains("GoogleCredentials(service_account, ****)"));
    assert!(!rendered.contains("fake@example.invalid"));
}

#[test]
fn google_pubsub_without_credentials_is_invalid() {
    let toml = r#"
[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
"#;
    let f = write_toml(toml);

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: missing credentials"))
    );
}

#[test]
fn google_pubsub_credentials_from_unset_env_is_invalid() {
    let f = write_toml(&google_pubsub_credentials_toml(
        r#"credentials = { env = "TROGON_GATEWAY_TEST_PUBSUB_CREDENTIALS_NOT_SET" }"#,
    ));

    let errors = validation_errors(load(Some(f.path())));

    assert!(errors.iter().any(|e| {
        e.contains("google_pubsub/primary: invalid credentials")
            && e.contains("TROGON_GATEWAY_TEST_PUBSUB_CREDENTIALS_NOT_SET")
    }));
}

#[test]
fn google_pubsub_credentials_that_are_not_json_are_invalid_without_echoing_them() {
    let f = write_toml(&google_pubsub_credentials_toml(
        r#"credentials = "leaky-marker-not-json""#,
    ));

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: invalid credentials"))
    );
    assert!(!errors.iter().any(|e| e.contains("leaky-marker-not-json")));
}

#[test]
fn google_pubsub_unsupported_credentials_type_is_invalid() {
    let f = write_toml(&google_pubsub_credentials_toml(
        r#"credentials = '{"type":"authorized_user","refresh_token":"leaky-refresh-marker"}'"#,
    ));

    let errors = validation_errors(load(Some(f.path())));

    assert!(errors.iter().any(
        |e| e.contains("google_pubsub/primary: invalid credentials") && e.contains("unsupported credentials type")
    ));
    assert!(!errors.iter().any(|e| e.contains("leaky-refresh-marker")));
}

#[test]
fn google_pubsub_reports_credentials_and_subscription_errors_together() {
    let f = write_toml(&google_pubsub_credentials_toml("").replace("projects/p/subscriptions/sub", "bad"));

    let errors = validation_errors(load(Some(f.path())));

    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: invalid subscription"))
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("google_pubsub/primary: missing credentials"))
    );
}

#[test]
fn plaid_resolves_with_defaults() {
    let f = write_toml(&plaid_toml("plaid-secret"));
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(cfg.plaid.len(), 1);
    let plaid = &cfg.plaid[0].config;
    assert_eq!(plaid.subject_prefix.as_str(), "plaid-primary");
    assert_eq!(plaid.stream_name.as_str(), "PLAID_PRIMARY");
    assert_eq!(std::time::Duration::from(plaid.timestamp_tolerance).as_secs(), 300);
    assert_eq!(plaid.client_id.as_str(), "plaid-client-id");
    assert_eq!(plaid.secret.as_str(), "plaid-secret");
    assert_eq!(
        plaid.api_base_url.webhook_verification_key_url().as_str(),
        "https://sandbox.plaid.com/webhook_verification_key/get"
    );
}

#[test]
fn plaid_production_environment_uses_the_production_api() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "production"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(
        cfg.plaid[0].config.api_base_url.webhook_verification_key_url().as_str(),
        "https://production.plaid.com/webhook_verification_key/get"
    );
}

#[test]
fn plaid_accepts_an_api_base_url_override() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
api_base_url = "http://127.0.0.1:9999"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(
        cfg.plaid[0].config.api_base_url.webhook_verification_key_url().as_str(),
        "http://127.0.0.1:9999/webhook_verification_key/get"
    );
}

#[test]
fn plaid_invalid_api_base_url_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
api_base_url = "ftp://example.com"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: invalid api_base_url")),
        "{errs:?}"
    );
}

#[test]
fn plaid_accepts_custom_timestamp_tolerance() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
timestamp_tolerance_secs = 60
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(
        std::time::Duration::from(cfg.plaid[0].config.timestamp_tolerance).as_secs(),
        60
    );
}

#[test]
fn plaid_resolves_secret_from_env_table() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = { env = "PATH" }
environment = "sandbox"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(!cfg.plaid[0].config.secret.as_str().is_empty());
}

#[test]
fn plaid_disabled_returns_none() {
    let toml = r#"
[sources.plaid]
status = "disabled"

[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.plaid.is_empty());
}

#[test]
fn plaid_integration_disabled_is_skipped() {
    let toml = r#"
[sources.plaid.integrations.primary]
status = "disabled"

[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.plaid.is_empty());
}

#[test]
fn plaid_missing_secret_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
environment = "sandbox"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary") && e.contains("secret")),
        "{errs:?}"
    );
}

#[test]
fn plaid_empty_secret_is_invalid() {
    let errs = validation_messages(&plaid_toml(""));
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: invalid secret")),
        "{errs:?}"
    );
}

#[test]
fn plaid_missing_client_id_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
secret = "plaid-secret"
environment = "sandbox"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter()
            .any(|e| e.contains("plaid/primary") && e.contains("client_id")),
        "{errs:?}"
    );
}

#[test]
fn plaid_empty_client_id_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "  "
secret = "plaid-secret"
environment = "sandbox"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: invalid client_id")),
        "{errs:?}"
    );
}

#[test]
fn plaid_resolves_client_id_from_env_table() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = { env = "TROGON_GATEWAY_TEST_PLAID_CLIENT_ID" }
secret = "plaid-secret"
environment = "sandbox"
"#;
    let f = write_toml(toml);
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("TROGON_GATEWAY_TEST_PLAID_CLIENT_ID", "plaid-client-id-from-env");
    let gateway_config = load_config::<GatewayConfig>(Some(f.path())).expect("load failed");
    let cfg = resolve(gateway_config, &NatsArgs::default(), &env).expect("resolve failed");
    assert_eq!(cfg.plaid[0].config.client_id.as_str(), "plaid-client-id-from-env");
}

#[test]
fn plaid_missing_client_id_env_is_reported_against_client_id() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = { env = "TROGON_GATEWAY_TEST_PLAID_CLIENT_ID_NOT_SET" }
secret = "plaid-secret"
environment = "sandbox"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: invalid client_id")
            && e.contains("TROGON_GATEWAY_TEST_PLAID_CLIENT_ID_NOT_SET")),
        "{errs:?}"
    );
}

#[test]
fn plaid_empty_client_id_env_name_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = { env = "  " }
secret = "plaid-secret"
environment = "sandbox"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: invalid client_id")),
        "{errs:?}"
    );
}

#[test]
fn identifier_input_returns_a_literal_unchanged() {
    let env = trogon_std::env::InMemoryEnv::new();
    let input = IdentifierInput::Literal("plaid-client-id".to_string());
    assert_eq!(input.resolve(&env).unwrap(), "plaid-client-id");
}

#[test]
fn identifier_input_reads_and_decodes_the_environment() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("PLAID_CLIENT_ID", "plain-id");
    env.set("PLAID_CLIENT_ID_ENCODED", "base64:cGxhaW4taWQ=");
    let plain = IdentifierInput::Env {
        env: " PLAID_CLIENT_ID ".to_string(),
    };
    let encoded = IdentifierInput::Env {
        env: "PLAID_CLIENT_ID_ENCODED".to_string(),
    };
    assert_eq!(plain.resolve(&env).unwrap(), "plain-id");
    assert_eq!(encoded.resolve(&env).unwrap(), "plain-id");
}

#[test]
fn identifier_input_rejects_a_blank_missing_or_invalid_environment() {
    let env = trogon_std::env::InMemoryEnv::new();
    env.set("PLAID_CLIENT_ID_BAD", "base64:!!!");
    let blank = IdentifierInput::Env { env: "  ".to_string() };
    let missing = IdentifierInput::Env {
        env: "PLAID_CLIENT_ID_MISSING".to_string(),
    };
    let invalid = IdentifierInput::Env {
        env: "PLAID_CLIENT_ID_BAD".to_string(),
    };
    assert!(matches!(blank.resolve(&env), Err(IdentifierInputError::EmptyEnvName)));
    match missing.resolve(&env) {
        Err(IdentifierInputError::MissingEnv { name }) => assert_eq!(name, "PLAID_CLIENT_ID_MISSING"),
        other => panic!("expected MissingEnv, got {other:?}"),
    }
    assert!(matches!(
        invalid.resolve(&env),
        Err(IdentifierInputError::InvalidValue(_))
    ));
}

#[test]
fn identifier_input_rejects_a_non_unicode_environment_variable() {
    let input = IdentifierInput::Env {
        env: "PLAID_CLIENT_ID".to_string(),
    };
    match input.resolve(&NonUnicodeEnv) {
        Err(IdentifierInputError::InvalidUnicodeEnv { name }) => assert_eq!(name, "PLAID_CLIENT_ID"),
        other => panic!("expected InvalidUnicodeEnv, got {other:?}"),
    }
}

#[test]
fn plaid_missing_environment_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter()
            .any(|e| e.contains("plaid/primary") && e.contains("environment")),
        "{errs:?}"
    );
}

#[test]
fn plaid_invalid_environment_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "development"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: invalid environment")),
        "{errs:?}"
    );
}

#[test]
fn plaid_invalid_subject_prefix() {
    let toml = r#"
[sources.plaid.integrations.primary]
subject_prefix = "bad prefix"

[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: invalid subject_prefix")),
        "{errs:?}"
    );
}

#[test]
fn plaid_zero_timestamp_tolerance_is_error() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
timestamp_tolerance_secs = 0
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter()
            .any(|e| e.contains("plaid/primary: timestamp_tolerance_secs must not be zero")),
        "{errs:?}"
    );
}

#[test]
fn plaid_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.plaid.integrations.primary]
stream_max_age_secs = 3600
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/primary: missing webhook")),
        "{errs:?}"
    );
}

#[test]
fn plaid_invalid_integration_id_is_invalid() {
    let toml = r#"
[sources.plaid.integrations."bad/id".webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter().any(|e| e.contains("plaid/bad/id: invalid integration id")),
        "{errs:?}"
    );
}

#[test]
fn plaid_unknown_webhook_field_is_rejected() {
    let toml = r#"
[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
verification_key = "nope"
"#;
    let f = write_toml(toml);
    assert!(load(Some(f.path())).is_err());
}

#[test]
fn zoom_resolves_with_defaults() {
    let f = write_toml(&zoom_toml("zoom-token"));
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(cfg.zoom.len(), 1);
    let zoom = &cfg.zoom[0].config;
    assert_eq!(zoom.subject_prefix.as_str(), "zoom-primary");
    assert_eq!(zoom.stream_name.as_str(), "ZOOM_PRIMARY");
    assert_eq!(std::time::Duration::from(zoom.timestamp_tolerance).as_secs(), 300);
    assert_eq!(zoom.secret_token.as_str(), "zoom-token");
}

#[test]
fn zoom_accepts_custom_timestamp_tolerance() {
    let toml = r#"
[sources.zoom.integrations.primary.webhook]
secret_token = "zoom-token"
timestamp_tolerance_secs = 60
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert_eq!(
        std::time::Duration::from(cfg.zoom[0].config.timestamp_tolerance).as_secs(),
        60
    );
}

#[test]
fn zoom_resolves_secret_token_from_env_table() {
    let toml = r#"
[sources.zoom.integrations.primary.webhook]
secret_token = { env = "PATH" }
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(!cfg.zoom[0].config.secret_token.as_str().is_empty());
}

#[test]
fn zoom_disabled_returns_none() {
    let toml = r#"
[sources.zoom]
status = "disabled"

[sources.zoom.integrations.primary.webhook]
secret_token = "zoom-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.zoom.is_empty());
}

#[test]
fn zoom_empty_secret_token_is_invalid() {
    let f = write_toml(&zoom_toml(""));
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("zoom/primary: invalid secret_token")))
    );
}

#[test]
fn zoom_missing_secret_token_is_invalid() {
    let toml = r#"
[sources.zoom.integrations.primary.webhook]
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("zoom/primary") && e.contains("secret_token")))
    );
}

#[test]
fn zoom_invalid_subject_prefix() {
    let toml = r#"
[sources.zoom.integrations.primary]
subject_prefix = "has.dots"

[sources.zoom.integrations.primary.webhook]
secret_token = "zoom-token"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("zoom/primary: invalid subject_prefix")))
    );
}

#[test]
fn zoom_zero_timestamp_tolerance_is_error() {
    let toml = r#"
[sources.zoom.integrations.primary.webhook]
secret_token = "zoom-token"
timestamp_tolerance_secs = 0
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("zoom/primary: timestamp_tolerance_secs must not be zero")))
    );
}

#[test]
fn zoom_enabled_integration_without_webhook_is_invalid() {
    let toml = r#"
[sources.zoom.integrations.primary]
status = "enabled"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("zoom/primary: missing webhook")))
    );
}

#[test]
fn zoom_invalid_integration_id_is_invalid() {
    let toml = r#"
[sources.zoom.integrations."bad/id".webhook]
secret_token = "zoom-token"
"#;
    let f = write_toml(toml);
    let result = load(Some(f.path()));
    assert!(
        matches!(result, Err(ConfigError::Validation(ref errs)) if errs.iter().any(|e| e.contains("zoom/bad/id: invalid integration id")))
    );
}

#[test]
fn zoom_integration_disabled_is_skipped() {
    let toml = r#"
[sources.zoom.integrations.primary]
status = "disabled"

[sources.zoom.integrations.primary.webhook]
secret_token = "zoom-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.zoom.is_empty());
}

struct NonUnicodeEnv;

impl ReadEnv for NonUnicodeEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotUnicode(std::ffi::OsString::from("raw")))
    }
}

#[test]
fn byte_count_input_rejects_a_blank_environment_variable_name() {
    let env = trogon_std::env::InMemoryEnv::new();
    let input = ByteCountInput::Env { env: "  ".to_string() };
    assert!(matches!(input.resolve(&env), Err(ByteCountInputError::EmptyEnvName)));
}

#[test]
fn byte_count_input_rejects_a_non_unicode_environment_variable() {
    let input = ByteCountInput::Env {
        env: "SLACK_BODY".to_string(),
    };
    match input.resolve(&NonUnicodeEnv) {
        Err(ByteCountInputError::InvalidUnicodeEnv { name }) => assert_eq!(name, "SLACK_BODY"),
        other => panic!("expected InvalidUnicodeEnv, got {other:?}"),
    }
}

const WEBHOOK_INTEGRATION_SOURCES: [(&str, &str); 13] = [
    ("github", "webhook_secret"),
    ("slack", "signing_secret"),
    ("telegram", "webhook_secret"),
    ("twitter", "consumer_secret"),
    ("gitlab", "signing_token"),
    ("linear", "webhook_secret"),
    ("microsoft_graph", "client_state"),
    ("incidentio", "signing_secret"),
    ("zoom", "secret_token"),
    ("plaid", "secret"),
    ("notion", "verification_token"),
    ("sentry", "client_secret"),
    ("datadog", "webhook_token"),
];

fn validation_messages(toml: &str) -> Vec<String> {
    let f = write_toml(toml);
    match load(Some(f.path())) {
        Err(ConfigError::Validation(errs)) => errs.iter().map(ToString::to_string).collect(),
        _ => panic!("expected validation errors"),
    }
}

#[test]
fn every_webhook_source_rejects_an_invalid_integration_id() {
    for (source, field) in WEBHOOK_INTEGRATION_SOURCES {
        let toml = format!("[sources.{source}.integrations.\"bad/id\".webhook]\n{field} = \"value\"\n");
        let errs = validation_messages(&toml);
        let expected = format!("{source}/bad/id: invalid integration id");
        assert!(errs.iter().any(|e| e.contains(&expected)), "{source}: {errs:?}");
    }
}

#[test]
fn every_webhook_source_skips_a_disabled_integration() {
    for (source, field) in WEBHOOK_INTEGRATION_SOURCES {
        let toml = format!(
            "[sources.{source}.integrations.primary]\nstatus = \"disabled\"\n\n[sources.{source}.integrations.primary.webhook]\n{field} = \"value\"\n"
        );
        let f = write_toml(&toml);
        let cfg = load(Some(f.path())).unwrap_or_else(|e| panic!("{source}: {e:?}"));
        assert!(!cfg.has_any_source(), "{source} should be skipped");
    }
}

#[test]
fn every_webhook_source_rejects_an_invalid_integration_status() {
    for (source, field) in WEBHOOK_INTEGRATION_SOURCES {
        let toml = format!(
            "[sources.{source}.integrations.primary]\nstatus = \"maybe\"\n\n[sources.{source}.integrations.primary.webhook]\n{field} = \"value\"\n"
        );
        let errs = validation_messages(&toml);
        let expected = format!("{source}/primary: invalid status");
        assert!(errs.iter().any(|e| e.contains(&expected)), "{source}: {errs:?}");
    }
}

#[test]
fn every_webhook_secret_that_decodes_to_nothing_is_rejected() {
    for (source, field) in WEBHOOK_INTEGRATION_SOURCES {
        let toml = format!("[sources.{source}.integrations.primary.webhook]\n{field} = \"base64:\"\n");
        let errs = validation_messages(&toml);
        let expected = format!("{source}/primary: invalid {field}");
        assert!(errs.iter().any(|e| e.contains(&expected)), "{source}: {errs:?}");
    }
}

#[test]
fn discord_bot_token_that_decodes_to_nothing_is_rejected() {
    let errs = validation_messages(&discord_gateway_toml("base64:"));
    assert!(
        errs.iter().any(|e| e.contains("discord: invalid bot_token")),
        "{errs:?}"
    );
}

#[test]
fn telegram_startup_registration_with_a_malformed_bot_token_is_invalid() {
    let toml = r#"
[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-webhook-secret"
webhook_registration_mode = "startup"
bot_token = "not-a-bot-token"
public_webhook_url = "https://example.com/sources/telegram/primary/webhook"
"#;
    let errs = validation_messages(toml);
    assert!(
        errs.iter()
            .any(|e| e.contains("telegram/primary: invalid bot_token") && e.contains("Telegram bot token format")),
        "{errs:?}"
    );
}
