use super::*;
    use crate::config::load;
    use crate::source::github::constants::{HEADER_DELIVERY, HEADER_EVENT, HEADER_SIGNATURE};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use hmac::{KeyInit, Mac};
    use std::io::Write;
    use tower::ServiceExt;
    use trogon_nats::jetstream::{ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore};

    type HmacSha256 = hmac::Hmac<sha2::Sha256>;

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

    fn compute_github_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn github_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(HEADER_EVENT, "push")
            .header(HEADER_DELIVERY, "delivery-1")
            .header(HEADER_SIGNATURE, compute_github_sig(secret, body))
            .body(Body::from(body.to_vec()))
            .unwrap()
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

    #[tokio::test]
    async fn github_webhook_integrations_route_by_integration_id() {
        let toml = r#"
[sources.github.integrations.acme-main.webhook]
webhook_secret = "acme-secret"

[sources.github.integrations.other.webhook]
webhook_secret = "other-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources(cfg, wrap_publisher(publisher.clone()));
        let body = br#"{"ref":"refs/heads/main"}"#;

        let source_prefix_response = app
            .clone()
            .oneshot(github_webhook_request("/github/acme-main/webhook", body, "acme-secret"))
            .await
            .unwrap();
        assert_eq!(source_prefix_response.status(), StatusCode::NOT_FOUND);
        assert!(publisher.published_messages().is_empty());

        let response = app
            .oneshot(github_webhook_request(
                "/sources/github/acme-main/webhook",
                body,
                "acme-secret",
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "github-acme-main.push");
    }

    #[tokio::test]
    async fn slack_socket_mode_integration_does_not_mount_webhook_route() {
        let toml = r#"
[sources.slack.integrations.primary.socket_mode]
app_token = "xapp-test-token"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let app = mount_sources(cfg, wrap_publisher(MockJetStreamPublisher::new()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sources/slack/primary/webhook")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
