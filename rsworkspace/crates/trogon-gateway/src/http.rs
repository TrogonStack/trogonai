use axum::Router;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use crate::config::ResolvedConfig;
use crate::source_plugin;

pub(crate) fn mount_sources<P, S>(config: ResolvedConfig, publisher: ClaimCheckPublisher<P, S>) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    let app = Router::new()
        .route(
            "/-/liveness",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/-/readiness",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        );

    source_plugin::mount_all(app, publisher, &config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;
    use std::io::Write;
    use trogon_nats::jetstream::{ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore};

    fn wrap_publisher(
        publisher: MockJetStreamPublisher,
    ) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
        ClaimCheckPublisher::new(
            publisher,
            MockObjectStore::new(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(usize::MAX),
        )
    }

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

[sources.notion]
verification_token = "notion-verification-token-example"

[sources.sentry]
client_secret = "sentry-client-secret"
"#
        .to_string()
    }

    #[test]
    fn mount_sources_with_no_sources_builds_router() {
        let cfg = load(None).expect("load failed");
        let _app = mount_sources(cfg, wrap_publisher(MockJetStreamPublisher::new()));
    }

    #[test]
    fn mount_sources_with_all_sources_builds_router() {
        let f = write_toml(&all_sources_toml());
        let cfg = load(Some(f.path())).expect("load failed");
        let _app = mount_sources(cfg, wrap_publisher(MockJetStreamPublisher::new()));
    }
}
