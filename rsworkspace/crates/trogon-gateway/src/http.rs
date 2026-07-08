use axum::Router;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use crate::config::ResolvedConfig;
use crate::secret_store::{SecretStoreError, SecretStoreGet};
use crate::source_plugin;
use crate::source_plugin::RuntimeCredentialMounts;

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

    source_plugin::mount_webhook_sources(app, publisher, &config)
}

pub(crate) fn mount_sources_with_runtime_credentials<P, S, G>(
    config: ResolvedConfig,
    publisher: ClaimCheckPublisher<P, S>,
    runtime_credentials: RuntimeCredentialMounts<G>,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    G: SecretStoreGet<Error = SecretStoreError>,
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

    source_plugin::mount_webhook_sources_with_runtime_credentials(app, publisher, &config, runtime_credentials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;
    use crate::credential::commands::domain::{CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
    use crate::credential::processor::runtime_projection::{
        RuntimeCredentialRegistry, RuntimeCredentialResolver, RuntimeIntegrationProjection,
    };
    use crate::secret_store::{MockOpenBaoSecretStore, SecretStorePut};
    use crate::source::github::constants::{HEADER_DELIVERY, HEADER_EVENT, HEADER_SIGNATURE};
    use crate::source::gitlab::GitLabSigningToken;
    use crate::source::gitlab::constants as gitlab_constants;
    use crate::source::incidentio::IncidentioSigningSecret;
    use crate::source::incidentio::constants as incidentio_constants;
    use crate::source::notion::NotionVerificationToken;
    use crate::source::notion::constants as notion_constants;
    use crate::source::sentry::constants as sentry_constants;
    use crate::source::slack::constants as slack_constants;
    use crate::source::telegram::constants as telegram_constants;
    use crate::source::twitter::constants as twitter_constants;
    use crate::source_integration_id::SourceIntegrationId;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
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

    fn compute_slack_sig(secret: &str, timestamp: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"v0:");
        mac.update(timestamp.as_bytes());
        mac.update(b":");
        mac.update(body);
        format!("v0={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn slack_timestamp() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn slack_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        let timestamp = slack_timestamp();
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(slack_constants::HEADER_TIMESTAMP, timestamp.as_str())
            .header(
                slack_constants::HEADER_SIGNATURE,
                compute_slack_sig(secret, &timestamp, body),
            )
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn incidentio_timestamp() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn compute_incidentio_sig(secret: &str, webhook_id: &str, timestamp: &str, body: &[u8]) -> String {
        let secret = IncidentioSigningSecret::new(secret).unwrap();
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        let signed = format!("{webhook_id}.{timestamp}.").into_bytes();
        mac.update(&signed);
        mac.update(body);
        format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn incidentio_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        let timestamp = incidentio_timestamp();
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(incidentio_constants::HEADER_WEBHOOK_ID, "msg-incidentio")
            .header(incidentio_constants::HEADER_WEBHOOK_TIMESTAMP, timestamp.as_str())
            .header(
                incidentio_constants::HEADER_WEBHOOK_SIGNATURE,
                compute_incidentio_sig(secret, "msg-incidentio", &timestamp, body),
            )
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn gitlab_timestamp() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string()
    }

    fn compute_gitlab_sig(token: &str, webhook_id: &str, timestamp: &str, body: &[u8]) -> String {
        let token = GitLabSigningToken::new(token).unwrap();
        let mut mac = HmacSha256::new_from_slice(token.as_bytes()).unwrap();
        let signed = format!("{webhook_id}.{timestamp}.").into_bytes();
        mac.update(&signed);
        mac.update(body);
        format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn gitlab_webhook_request(uri: &str, body: &[u8], token: &str) -> Request<Body> {
        let timestamp = gitlab_timestamp();
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(gitlab_constants::HEADER_EVENT, "push")
            .header(gitlab_constants::HEADER_WEBHOOK_UUID, "wh-uuid-test")
            .header(gitlab_constants::HEADER_IDEMPOTENCY_KEY, "idem-key-test")
            .header(gitlab_constants::HEADER_INSTANCE, "https://gitlab.example.com")
            .header(gitlab_constants::HEADER_EVENT_UUID, "evt-uuid-test")
            .header(gitlab_constants::HEADER_WEBHOOK_ID, "msg-gitlab")
            .header(gitlab_constants::HEADER_WEBHOOK_TIMESTAMP, timestamp.as_str())
            .header(
                gitlab_constants::HEADER_WEBHOOK_SIGNATURE,
                compute_gitlab_sig(token, "msg-gitlab", &timestamp, body),
            )
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn compute_linear_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn linear_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("linear-signature", compute_linear_sig(secret, body))
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn sentry_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(sentry_constants::HEADER_RESOURCE, "issue")
            .header(sentry_constants::HEADER_TIMESTAMP, "1711315768")
            .header(sentry_constants::HEADER_REQUEST_ID, "req-1")
            .header(sentry_constants::HEADER_SIGNATURE, compute_linear_sig(secret, body))
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn twitter_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(twitter_constants::HEADER_SIGNATURE, compute_twitter_sig(secret, body))
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn compute_twitter_sig(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn notion_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(notion_constants::HEADER_SIGNATURE, compute_notion_sig(secret, body))
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    fn compute_notion_sig(secret: &str, body: &[u8]) -> String {
        let token = NotionVerificationToken::new(secret).unwrap();
        let mut mac = HmacSha256::new_from_slice(token.as_str().as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    fn microsoft_graph_webhook_request(uri: &str, client_state: &str) -> Request<Body> {
        let body = serde_json::to_vec(&serde_json::json!({
            "value": [{
                "id": "notification-1",
                "subscriptionId": "subscription-1",
                "clientState": client_state
            }]
        }))
        .unwrap();
        Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::from(body))
            .unwrap()
    }

    fn telegram_webhook_request(uri: &str, body: &[u8], secret: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(telegram_constants::HEADER_SECRET_TOKEN, secret)
            .body(Body::from(body.to_vec()))
            .unwrap()
    }

    async fn github_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(
            SourceKind::GitHub,
            CredentialKind::WebhookSecret,
            integration_id,
            secret,
        )
        .await
    }

    async fn slack_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(SourceKind::Slack, CredentialKind::SigningSecret, integration_id, secret).await
    }

    async fn gitlab_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(SourceKind::Gitlab, CredentialKind::SigningToken, integration_id, secret).await
    }

    async fn incidentio_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(
            SourceKind::Incidentio,
            CredentialKind::SigningSecret,
            integration_id,
            secret,
        )
        .await
    }

    async fn linear_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(
            SourceKind::Linear,
            CredentialKind::WebhookSecret,
            integration_id,
            secret,
        )
        .await
    }

    async fn sentry_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(SourceKind::Sentry, CredentialKind::ClientSecret, integration_id, secret).await
    }

    async fn notion_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(
            SourceKind::Notion,
            CredentialKind::VerificationToken,
            integration_id,
            secret,
        )
        .await
    }

    async fn microsoft_graph_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(
            SourceKind::MicrosoftGraph,
            CredentialKind::ClientState,
            integration_id,
            secret,
        )
        .await
    }

    async fn telegram_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(
            SourceKind::Telegram,
            CredentialKind::WebhookSecret,
            integration_id,
            secret,
        )
        .await
    }

    async fn twitter_runtime_credentials(
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        runtime_credentials(
            SourceKind::Twitter,
            CredentialKind::ConsumerSecret,
            integration_id,
            secret,
        )
        .await
    }

    async fn runtime_credentials(
        source: SourceKind,
        kind: CredentialKind,
        integration_id: &SourceIntegrationId,
        secret: &str,
    ) -> RuntimeCredentialResolver<MockOpenBaoSecretStore> {
        let store = MockOpenBaoSecretStore::default();
        let credential = store
            .put(
                CredentialScope::integration(
                    CredentialOwnerId::new("tenant-1").unwrap(),
                    source,
                    integration_id.clone(),
                ),
                kind,
                trogon_std::SecretString::new(secret).unwrap(),
            )
            .await
            .unwrap();
        let registry = RuntimeCredentialRegistry::default();
        registry
            .projections()
            .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential, 1).unwrap())
            .await;
        registry.resolver(store)
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
    async fn github_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.github.integrations.acme-main.webhook]
webhook_secret = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                github: Some(github_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = br#"{"ref":"refs/heads/main"}"#;

        let static_response = app
            .clone()
            .oneshot(github_webhook_request(
                "/sources/github/acme-main/webhook",
                body,
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(github_webhook_request(
                "/sources/github/acme-main/webhook",
                body,
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "github-acme-main.push");
    }

    #[tokio::test]
    async fn gitlab_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.gitlab.integrations.acme-main.webhook]
signing_token = "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                gitlab: Some(
                    gitlab_runtime_credentials(&integration_id, "whsec_YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY=")
                        .await,
                ),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = br#"{"ref":"refs/heads/main"}"#;

        let static_response = app
            .clone()
            .oneshot(gitlab_webhook_request(
                "/sources/gitlab/acme-main/webhook",
                body,
                "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(gitlab_webhook_request(
                "/sources/gitlab/acme-main/webhook",
                body,
                "whsec_YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY=",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "gitlab-acme-main.push");
    }

    #[tokio::test]
    async fn incidentio_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.incidentio.integrations.acme-main.webhook]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                incidentio: Some(incidentio_runtime_credentials(&integration_id, "whsec_cnVudGltZS1zZWNyZXQ=").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = br#"{"event_type":"public_incident.incident_created_v2","data":{"id":"01ABC"}}"#;

        let static_response = app
            .clone()
            .oneshot(incidentio_webhook_request(
                "/sources/incidentio/acme-main/webhook",
                body,
                "whsec_dGVzdC1zZWNyZXQ=",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(incidentio_webhook_request(
                "/sources/incidentio/acme-main/webhook",
                body,
                "whsec_cnVudGltZS1zZWNyZXQ=",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].subject,
            "incidentio-acme-main.public_incident.incident_created_v2"
        );
    }

    #[tokio::test]
    async fn slack_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.slack.integrations.acme-main.webhook]
signing_secret = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                slack: Some(slack_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "event_callback",
            "event_id": "Ev01ABC123",
            "team_id": "T01ABC",
            "event": { "type": "message", "text": "hello" }
        }))
        .unwrap();

        let static_response = app
            .clone()
            .oneshot(slack_webhook_request(
                "/sources/slack/acme-main/webhook",
                &body,
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(slack_webhook_request(
                "/sources/slack/acme-main/webhook",
                &body,
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "slack-acme-main.event.message");
    }

    #[tokio::test]
    async fn linear_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.linear.integrations.acme-main.webhook]
webhook_secret = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                linear: Some(linear_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "Issue",
            "action": "create",
            "webhookId": "wh_test_123",
            "webhookTimestamp": now_ms,
            "data": {}
        }))
        .unwrap();

        let static_response = app
            .clone()
            .oneshot(linear_webhook_request(
                "/sources/linear/acme-main/webhook",
                &body,
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(linear_webhook_request(
                "/sources/linear/acme-main/webhook",
                &body,
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "linear-acme-main.Issue.create");
    }

    #[tokio::test]
    async fn microsoft_graph_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.microsoft_graph.integrations.acme-main.webhook]
client_state = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                microsoft_graph: Some(microsoft_graph_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );

        let static_response = app
            .clone()
            .oneshot(microsoft_graph_webhook_request(
                "/sources/microsoft-graph/acme-main/webhook",
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(microsoft_graph_webhook_request(
                "/sources/microsoft-graph/acme-main/webhook",
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::ACCEPTED);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].subject,
            "microsoft-graph-acme-main.change_notification_collection"
        );
    }

    #[tokio::test]
    async fn sentry_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.sentry.integrations.acme-main.webhook]
client_secret = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                sentry: Some(sentry_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = br#"{"action":"created","data":{}}"#;

        let static_response = app
            .clone()
            .oneshot(sentry_webhook_request(
                "/sources/sentry/acme-main/webhook",
                body,
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(sentry_webhook_request(
                "/sources/sentry/acme-main/webhook",
                body,
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "sentry-acme-main.issue.created");
    }

    #[tokio::test]
    async fn notion_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.notion.integrations.acme-main.webhook]
verification_token = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                notion: Some(notion_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "evt-1",
            "subscription_id": "subscription-1",
            "attempt_number": 1,
            "type": "page.created"
        }))
        .unwrap();

        let static_response = app
            .clone()
            .oneshot(notion_webhook_request(
                "/sources/notion/acme-main/webhook",
                &body,
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(notion_webhook_request(
                "/sources/notion/acme-main/webhook",
                &body,
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "notion-acme-main.page.created");
    }

    #[tokio::test]
    async fn telegram_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.telegram.integrations.acme-main.webhook]
webhook_secret = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                telegram: Some(telegram_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = serde_json::to_vec(&serde_json::json!({
            "update_id": 12345,
            "message": { "chat": { "id": 1 } }
        }))
        .unwrap();

        let static_response = app
            .clone()
            .oneshot(telegram_webhook_request(
                "/sources/telegram/acme-main/webhook",
                &body,
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(telegram_webhook_request(
                "/sources/telegram/acme-main/webhook",
                &body,
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "telegram-acme-main.message");
    }

    #[tokio::test]
    async fn twitter_webhook_can_resolve_secret_from_runtime_credentials() {
        let toml = r#"
[sources.twitter.integrations.acme-main.webhook]
consumer_secret = "static-secret"
"#;
        let f = write_toml(toml);
        let cfg = load(Some(f.path())).expect("load failed");
        let integration_id = SourceIntegrationId::new("acme-main").unwrap();
        let publisher = MockJetStreamPublisher::new();
        let app = mount_sources_with_runtime_credentials(
            cfg,
            wrap_publisher(publisher.clone()),
            RuntimeCredentialMounts {
                twitter: Some(twitter_runtime_credentials(&integration_id, "runtime-secret").await),
                ..RuntimeCredentialMounts::default()
            },
        );
        let body = br#"{
            "data": {
                "event_type": "profile.update.bio"
            }
        }"#;

        let static_response = app
            .clone()
            .oneshot(twitter_webhook_request(
                "/sources/twitter/acme-main/webhook",
                body,
                "static-secret",
            ))
            .await
            .unwrap();
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        assert!(publisher.published_messages().is_empty());

        let runtime_response = app
            .oneshot(twitter_webhook_request(
                "/sources/twitter/acme-main/webhook",
                body,
                "runtime-secret",
            ))
            .await
            .unwrap();

        assert_eq!(runtime_response.status(), StatusCode::OK);
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "twitter-acme-main.profile.update.bio");
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
}
