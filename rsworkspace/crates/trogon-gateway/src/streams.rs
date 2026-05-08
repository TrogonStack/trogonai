use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use crate::config::ResolvedConfig;

pub(crate) async fn provision<C: JetStreamContext>(client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
    if let Some(ref cfg) = config.github {
        crate::source::github::provision(client, cfg).await?;
        info!(source = "github", "stream provisioned");
    }
    if let Some(ref cfg) = config.discord {
        crate::source::discord::provision(client, cfg).await?;
        info!(source = "discord", "stream provisioned");
    }
    if let Some(ref cfg) = config.slack {
        crate::source::slack::provision(client, cfg).await?;
        info!(source = "slack", "stream provisioned");
    }
    if let Some(ref cfg) = config.telegram {
        crate::source::telegram::provision(client, cfg).await?;
        info!(source = "telegram", "stream provisioned");
    }
    if let Some(ref cfg) = config.twitter {
        crate::source::twitter::provision(client, cfg).await?;
        info!(source = "twitter", "stream provisioned");
    }
    if let Some(ref cfg) = config.gitlab {
        crate::source::gitlab::provision(client, cfg).await?;
        info!(source = "gitlab", "stream provisioned");
    }
    if let Some(ref cfg) = config.incidentio {
        crate::source::incidentio::provision(client, cfg).await?;
        info!(source = "incidentio", "stream provisioned");
    }
    if let Some(ref cfg) = config.linear {
        crate::source::linear::provision(client, cfg).await?;
        info!(source = "linear", "stream provisioned");
    }
    if let Some(ref cfg) = config.microsoft_graph {
        crate::source::microsoft_graph::provision(client, cfg).await?;
        info!(source = "microsoft-graph", "stream provisioned");
    }
    if let Some(ref cfg) = config.notion {
        crate::source::notion::provision(client, cfg).await?;
        info!(source = "notion", "stream provisioned");
    }
    if let Some(ref cfg) = config.sentry {
        crate::source::sentry::provision(client, cfg).await?;
        info!(source = "sentry", "stream provisioned");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
[sources.github]
webhook_secret = "gh-secret"

[sources.discord]
bot_token = "Bot token"

[sources.slack]
signing_secret = "slack-secret"

[sources.telegram]
webhook_secret = "tg-secret"

[sources.twitter]
consumer_secret = "twitter-consumer-secret"

[sources.gitlab]
webhook_secret = "gl-secret"

[sources.incidentio]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="

[sources.linear]
webhook_secret = "linear-secret"

[sources.microsoft_graph]
client_state = "microsoft-graph-client-state"

[sources.notion]
verification_token = "notion-verification-token-example"

[sources.sentry]
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
    async fn provision_skips_disabled_sources() {
        let toml = r#"
[sources.github]
status = "disabled"
webhook_secret = "gh-secret"

[sources.discord]
bot_token = "Bot token"

[sources.slack]
signing_secret = "slack-secret"

[sources.telegram]
webhook_secret = "tg-secret"

[sources.twitter]
consumer_secret = "twitter-consumer-secret"

[sources.gitlab]
webhook_secret = "gl-secret"

[sources.incidentio]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="

[sources.linear]
webhook_secret = "linear-secret"

[sources.microsoft_graph]
client_state = "microsoft-graph-client-state"

[sources.notion]
verification_token = "notion-verification-token-example"

[sources.sentry]
client_secret = "sentry-client-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let js = MockJetStreamContext::new();

        provision(&js, &cfg).await.expect("provision should succeed");

        assert_eq!(js.created_streams().len(), 10);
    }
}
