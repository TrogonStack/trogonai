use super::*;
use crate::config::load;
use crate::credential::constants::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};
use std::io::Write;
use trogon_nats::jetstream::MockJetStreamContext;

fn write_toml(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("failed to create temp file");
    f.write_all(content.as_bytes()).expect("failed to write toml");
    f.flush().expect("failed to flush");
    f
}

fn all_sources_toml() -> String {
    r#"
[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"

[sources.discord]
bot_token = "Bot token"

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"

[sources.telegram.integrations.primary.webhook]
webhook_secret = "tg-secret"

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"

[sources.gitlab.integrations.primary.webhook]
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="

[sources.incidentio.integrations.primary.webhook]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="

[sources.linear.integrations.primary.webhook]
webhook_secret = "linear-secret"

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/p/subscriptions/sub"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"

[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"

[sources.zoom.integrations.primary.webhook]
secret_token = "zoom-secret-token"

[sources.plaid.integrations.primary.webhook]
client_id = "plaid-client-id"
secret = "plaid-secret"
environment = "sandbox"
"#
    .to_string()
}

#[tokio::test]
async fn provision_no_sources_creates_credential_stream() {
    let cfg = load(None).expect("load failed");
    let js = MockJetStreamContext::new();

    provision(&js, &cfg).await.expect("provision should succeed");

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, CREDENTIAL_STREAM);
    assert_eq!(
        streams[0].subjects,
        vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")]
    );
    assert!(streams[0].allow_atomic_publish);
}

#[tokio::test]
async fn provision_all_sources_creates_all_streams() {
    let f = write_toml(&all_sources_toml());
    let cfg = load(Some(f.path())).expect("load failed");
    let js = MockJetStreamContext::new();

    provision(&js, &cfg).await.expect("provision should succeed");

    assert_eq!(js.created_streams().len(), 16);
}

#[tokio::test]
async fn provision_source_integrations_creates_integration_streams() {
    let toml = r#"
[sources.github.integrations.acme-main.webhook]
webhook_secret = "acme-secret"

[sources.github.integrations.acme_main.webhook]
webhook_secret = "underscore-secret"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    let js = MockJetStreamContext::new();

    provision(&js, &cfg).await.expect("provision should succeed");

    let streams = js.created_streams();
    assert_eq!(streams.len(), 3);
    assert_eq!(streams[0].name, CREDENTIAL_STREAM);
    assert_eq!(streams[1].name, "GITHUB_ACME-MAIN");
    assert_eq!(streams[1].subjects, vec!["github-acme-main.>"]);
    assert_eq!(streams[2].name, "GITHUB_ACME_MAIN");
    assert_eq!(streams[2].subjects, vec!["github-acme_main.>"]);
}

#[tokio::test]
async fn provision_skips_disabled_sources() {
    let toml = r#"
[sources.github]
status = "disabled"

[sources.github.integrations.primary.webhook]
webhook_secret = "gh-secret"

[sources.discord]
bot_token = "Bot token"

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"

[sources.telegram.integrations.primary.webhook]
webhook_secret = "tg-secret"

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"

[sources.gitlab.integrations.primary.webhook]
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="

[sources.incidentio.integrations.primary.webhook]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="

[sources.linear.integrations.primary.webhook]
webhook_secret = "linear-secret"

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"

[sources.datadog.integrations.primary.webhook]
webhook_token = "datadog-webhook-token"
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    let js = MockJetStreamContext::new();

    provision(&js, &cfg).await.expect("provision should succeed");

    assert_eq!(js.created_streams().len(), 12);
}

#[tokio::test]
async fn provision_google_pubsub_creates_a_stream_per_integration() {
    let toml = r#"
[sources.google_pubsub.integrations.orders.streaming_pull]
subscription = "projects/p/subscriptions/orders"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'

[sources.google_pubsub.integrations.billing]
subject_prefix = "billing-events"
stream_name = "BILLING_EVENTS"

[sources.google_pubsub.integrations.billing.streaming_pull]
subscription = "projects/p/subscriptions/billing"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#;
    let f = write_toml(toml);
    let cfg = load(Some(f.path())).expect("load failed");
    let js = MockJetStreamContext::new();

    provision(&js, &cfg).await.expect("provision should succeed");

    let streams = js.created_streams();
    assert_eq!(streams.len(), 3);
    assert_eq!(streams[0].name, CREDENTIAL_STREAM);
    assert_eq!(streams[1].name, "BILLING_EVENTS");
    assert_eq!(streams[1].subjects, vec!["billing-events.>"]);
    assert_eq!(streams[2].name, "GOOGLE_PUBSUB_ORDERS");
    assert_eq!(streams[2].subjects, vec!["google-pubsub-orders.>"]);
}

#[tokio::test]
async fn provision_google_pubsub_propagates_stream_errors() {
    let f = write_toml(
        r#"
[sources.google_pubsub.integrations.orders.streaming_pull]
subscription = "projects/p/subscriptions/orders"
credentials = '{"type":"service_account","project_id":"fake","private_key_id":"fake","private_key":"fake","client_email":"fake@example.invalid"}'
"#,
    );
    let cfg = load(Some(f.path())).expect("load failed");
    let js = MockJetStreamContext::new();
    js.fail_next();

    assert!(provision(&js, &cfg).await.is_err());
}
