use axum::Router;
use tracing::info;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use crate::config::ResolvedConfig;

pub(crate) fn mount_sources<P, S, R>(
    config: ResolvedConfig,
    publisher: ClaimCheckPublisher<P, S>,
    nats: R,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    R: trogon_nats::RequestClient,
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
        app = app.nest(
            "/github",
            trogon_source_github::router(publisher.clone(), cfg),
        );
        info!(source = "github", "mounted at /github");
    }

    if let Some(ref cfg) = config.discord
        && let trogon_source_discord::config::SourceMode::Webhook { public_key } = cfg.mode
    {
        let sub = trogon_source_discord::router(publisher.clone(), nats.clone(), public_key, cfg);

        app = app.nest("/discord", sub);
        info!(source = "discord", "mounted at /discord");
    }

    if let Some(ref cfg) = config.slack {
        app = app.nest(
            "/slack",
            trogon_source_slack::router(publisher.clone(), cfg),
        );
        info!(source = "slack", "mounted at /slack");
    }

    if let Some(ref cfg) = config.telegram {
        app = app.nest(
            "/telegram",
            trogon_source_telegram::router(publisher.clone(), cfg),
        );
        info!(source = "telegram", "mounted at /telegram");
    }

    if let Some(ref cfg) = config.gitlab {
        app = app.nest(
            "/gitlab",
            trogon_source_gitlab::router(publisher.clone(), cfg),
        );
        info!(source = "gitlab", "mounted at /gitlab");
    }

    if let Some(ref cfg) = config.incidentio {
        app = app.nest(
            "/incidentio",
            trogon_source_incidentio::router(publisher.clone(), cfg),
        );
        info!(source = "incidentio", "mounted at /incidentio");
    }

    if let Some(ref cfg) = config.linear {
        app = app.nest(
            "/linear",
            trogon_source_linear::router(publisher.clone(), cfg),
        );
        info!(source = "linear", "mounted at /linear");
    }

    app
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;
    use std::io::Write;
    use trogon_nats::MockNatsClient;
    use trogon_nats::jetstream::{
        ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore,
    };

    const VALID_ED25519_PUB_KEY: &str =
        "236a4d1cb6b5d3b6e25664d96be99807095ea11930159bb832e53b87761648c3";

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

[sources.incidentio]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="

[sources.linear]
webhook_secret = "linear-secret"
"#
        )
    }

    #[test]
    fn mount_sources_with_no_sources_builds_router() {
        let cfg = load(None).expect("load failed");
        let _app = mount_sources(
            cfg,
            wrap_publisher(MockJetStreamPublisher::new()),
            MockNatsClient::new(),
        );
    }

    #[test]
    fn mount_sources_with_all_sources_builds_router() {
        let f = write_toml(&all_sources_toml());
        let cfg = load(Some(f.path())).expect("load failed");
        let _app = mount_sources(
            cfg,
            wrap_publisher(MockJetStreamPublisher::new()),
            MockNatsClient::new(),
        );
    }
}
