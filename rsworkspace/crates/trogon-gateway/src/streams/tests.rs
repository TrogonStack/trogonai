use super::*;
    use crate::config::load;
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

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
"#
        .to_string()
    }

    #[tokio::test]
    async fn provision_no_sources_is_noop() {
        let cfg = load(None).expect("load failed");
        let js = MockJetStreamContext::new();

        provision(&js, &cfg).await.expect("provision should succeed");

        assert!(js.created_streams().is_empty());
    }

    #[tokio::test]
    async fn provision_all_sources_creates_all_streams() {
        let f = write_toml(&all_sources_toml());
        let cfg = load(Some(f.path())).expect("load failed");
        let js = MockJetStreamContext::new();

        provision(&js, &cfg).await.expect("provision should succeed");

        assert_eq!(js.created_streams().len(), 11);
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
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].name, "GITHUB_ACME-MAIN");
        assert_eq!(streams[0].subjects, vec!["github-acme-main.>"]);
        assert_eq!(streams[1].name, "GITHUB_ACME_MAIN");
        assert_eq!(streams[1].subjects, vec!["github-acme_main.>"]);
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
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let js = MockJetStreamContext::new();

        provision(&js, &cfg).await.expect("provision should succeed");

        assert_eq!(js.created_streams().len(), 10);
    }
