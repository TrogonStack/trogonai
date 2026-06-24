use super::*;
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
    assert!(cfg.slack[0].config.webhook().is_some());
    assert!(cfg.slack[0].config.socket_mode().is_none());
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
app_token = "xapp-test-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    assert!(cfg.slack.is_empty());
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
fn slack_integration_rejects_webhook_and_socket_mode_together() {
    let toml = r#"
[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"

[sources.slack.integrations.primary.socket_mode]
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
    let err = ConfigError::Validation(ValidationErrors(vec![
        ConfigValidationError::invalid("discord", "stream_max_age_secs", ZeroDuration),
        ConfigValidationError::invalid_subject_token(
            "discord",
            "subject_prefix",
            SubjectTokenViolation::InvalidCharacter('.'),
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
    let err = ConfigError::Validation(ValidationErrors(vec![ConfigValidationError::invalid(
        "discord",
        "stream_max_age_secs",
        ZeroDuration,
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
