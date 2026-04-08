use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use crate::config::ResolvedConfig;

pub(crate) async fn provision<C: JetStreamContext>(
    client: &C,
    config: &ResolvedConfig,
) -> Result<(), C::Error> {
    if let Some(ref cfg) = config.github {
        trogon_source_github::provision(client, cfg).await?;
        info!(source = "github", "stream provisioned");
    }
    if let Some(ref cfg) = config.discord {
        trogon_source_discord::provision(client, cfg).await?;
        info!(source = "discord", "stream provisioned");
    }
    if let Some(ref cfg) = config.slack {
        trogon_source_slack::provision(client, cfg).await?;
        info!(source = "slack", "stream provisioned");
    }
    if let Some(ref cfg) = config.telegram {
        trogon_source_telegram::provision(client, cfg).await?;
        info!(source = "telegram", "stream provisioned");
    }
    if let Some(ref cfg) = config.gitlab {
        trogon_source_gitlab::provision(client, cfg).await?;
        info!(source = "gitlab", "stream provisioned");
    }
    if let Some(ref cfg) = config.linear {
        trogon_source_linear::provision(client, cfg).await?;
        info!(source = "linear", "stream provisioned");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;
    use std::io::Write;
    use trogon_nats::jetstream::MockJetStreamContext;

    const VALID_ED25519_PUB_KEY: &str =
        "236a4d1cb6b5d3b6e25664d96be99807095ea11930159bb832e53b87761648c3";

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

    fn all_sources_toml() -> String {
        format!(
            r#"
[sources.github]
webhook_secret = "gh-secret"

[sources.discord]
mode = "webhook"
public_key = "{VALID_ED25519_PUB_KEY}"

[sources.slack]
signing_secret = "slack-secret"

[sources.telegram]
webhook_secret = "tg-secret"

[sources.gitlab]
webhook_secret = "gl-secret"

[sources.linear]
webhook_secret = "linear-secret"
"#
        )
    }

    #[tokio::test]
    async fn provision_no_sources_is_noop() {
        let cfg = load(None).expect("load failed");
        let js = MockJetStreamContext::new();

        provision(&js, &cfg)
            .await
            .expect("provision should succeed");

        assert!(js.created_streams().is_empty());
    }

    #[tokio::test]
    async fn provision_all_sources_creates_all_streams() {
        let f = write_toml(&all_sources_toml());
        let cfg = load(Some(f.path())).expect("load failed");
        let js = MockJetStreamContext::new();

        provision(&js, &cfg)
            .await
            .expect("provision should succeed");

        assert_eq!(js.created_streams().len(), 6);
    }
}
