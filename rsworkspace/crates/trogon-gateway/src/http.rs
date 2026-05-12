use axum::Router;
use tracing::info;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use crate::config::ResolvedConfig;

pub(crate) fn mount_sources<P, S>(config: ResolvedConfig, publisher: ClaimCheckPublisher<P, S>) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    let mut app = Router::new()
        .route(
            "/-/liveness",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/-/readiness",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        );

    if let Some(ref cfg) = config.github {
        app = app.nest("/github", crate::source::github::router(publisher.clone(), cfg));
        info!(source = "github", "mounted at /github");
    }

    if let Some(ref cfg) = config.slack {
        app = app.nest("/slack", crate::source::slack::router(publisher.clone(), cfg));
        info!(source = "slack", "mounted at /slack");
    }

    if let Some(ref cfg) = config.telegram {
        app = app.nest("/telegram", crate::source::telegram::router(publisher.clone(), cfg));
        info!(source = "telegram", "mounted at /telegram");
    }

    if let Some(ref cfg) = config.twitter {
        app = app.nest("/twitter", crate::source::twitter::router(publisher.clone(), cfg));
        info!(source = "twitter", "mounted at /twitter");
    }

    if let Some(ref cfg) = config.gitlab {
        app = app.nest("/gitlab", crate::source::gitlab::router(publisher.clone(), cfg));
        info!(source = "gitlab", "mounted at /gitlab");
    }

    if let Some(ref cfg) = config.incidentio {
        app = app.nest("/incidentio", crate::source::incidentio::router(publisher.clone(), cfg));
        info!(source = "incidentio", "mounted at /incidentio");
    }

    if let Some(ref cfg) = config.linear {
        app = app.nest("/linear", crate::source::linear::router(publisher.clone(), cfg));
        info!(source = "linear", "mounted at /linear");
    }

    if let Some(ref cfg) = config.microsoft_graph {
        app = app.nest(
            "/microsoft-graph",
            crate::source::microsoft_graph::router(publisher.clone(), cfg),
        );
        info!(source = "microsoft-graph", "mounted at /microsoft-graph");
    }

    if let Some(ref cfg) = config.notion {
        app = app.nest("/notion", crate::source::notion::router(publisher.clone(), cfg));
        info!(source = "notion", "mounted at /notion");
    }

    if let Some(ref cfg) = config.sentry {
        app = app.nest("/sentry", crate::source::sentry::router(publisher.clone(), cfg));
        info!(source = "sentry", "mounted at /sentry");
    }

    app
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

[sources.microsoft_graph]
client_state = "microsoft-graph-client-state"

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
