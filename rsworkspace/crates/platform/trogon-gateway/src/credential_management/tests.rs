use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request};
use chrono::Utc;
use serde_json::Value;
use tokio::sync::Mutex;
use tower::ServiceExt;
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, Snapshot, SnapshotRead, SnapshotWrite, StreamAppend, StreamEvent, StreamPosition, StreamRead,
    StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
};

use super::*;
use crate::credential_lifecycle::recovery_worker::{
    CredentialRecoveryCheckpoint, CredentialRecoveryCheckpointStoreError,
};
use crate::credential_runtime_projection::{RuntimeCredentialError, RuntimeIntegrationKey};
use crate::secret_store::MockOpenBaoSecretStore;
use trogonai_proto::gateway::credentials::v1;

#[derive(Clone, Default)]
struct ManagementTestStreamStore {
    events: Arc<Mutex<Vec<StreamEvent>>>,
    snapshots: Arc<Mutex<BTreeMap<String, Snapshot<state_v1::CredentialStateSnapshot>>>>,
}

#[derive(Debug, thiserror::Error)]
#[error("management test stream rejected the append")]
struct ManagementTestStreamError;

impl ManagementTestStreamStore {
    async fn decoded_events(&self) -> Vec<v1::CredentialEvent> {
        let events = self.events.lock().await.clone();
        events
            .into_iter()
            .filter_map(|event| event.decode::<v1::CredentialEvent>().unwrap().into_decoded())
            .collect()
    }
}

impl StreamRead<str> for ManagementTestStreamStore {
    type Error = ManagementTestStreamError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let start = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let events = self.events.lock().await;
        let stream_events = events
            .iter()
            .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
            .cloned()
            .collect();
        Ok(ReadStreamResponse {
            current_position: current_position(&events, request.stream_id),
            events: stream_events,
        })
    }
}

impl StreamAppend<str> for ManagementTestStreamStore {
    type Error = ManagementTestStreamError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let mut events = self.events.lock().await;
        let current_position = current_position(&events, request.stream_id);
        match request.stream_write_precondition {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if current_position.is_some() => {}
            StreamWritePrecondition::NoStream if current_position.is_none() => {}
            StreamWritePrecondition::At(position) if current_position == Some(position) => {}
            _ => return Err(ManagementTestStreamError),
        }

        let mut last_position = current_position;
        for event in request.events {
            let stream_position = position(events.len() as u64 + 1);
            last_position = Some(stream_position);
            events.push(StreamEvent {
                stream_id: request.stream_id.to_string(),
                event,
                stream_position,
                recorded_at: Utc::now(),
            });
        }

        Ok(AppendStreamResponse {
            stream_position: last_position.expect("append request must contain events"),
        })
    }
}

impl SnapshotRead<state_v1::CredentialStateSnapshot, str> for ManagementTestStreamStore {
    type Error = ManagementTestStreamError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<state_v1::CredentialStateSnapshot>, Self::Error> {
        let snapshots = self.snapshots.lock().await;
        Ok(ReadSnapshotResponse {
            snapshot: snapshots.get(request.snapshot_id).cloned(),
        })
    }
}

impl SnapshotWrite<state_v1::CredentialStateSnapshot, str> for ManagementTestStreamStore {
    type Error = ManagementTestStreamError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, state_v1::CredentialStateSnapshot, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        let mut snapshots = self.snapshots.lock().await;
        snapshots.insert(request.snapshot_id.to_string(), request.snapshot);
        Ok(WriteSnapshotResponse)
    }
}

#[derive(Clone, Default)]
struct ManagementTestRecoveryCheckpointStore {
    checkpoint: Arc<Mutex<CredentialRecoveryCheckpoint>>,
}

impl ManagementTestRecoveryCheckpointStore {
    async fn with_checkpoint(checkpoint: CredentialRecoveryCheckpoint) -> Self {
        Self {
            checkpoint: Arc::new(Mutex::new(checkpoint)),
        }
    }
}

impl CredentialRecoveryCheckpointStore for ManagementTestRecoveryCheckpointStore {
    async fn load(&self) -> Result<CredentialRecoveryCheckpoint, CredentialRecoveryCheckpointStoreError> {
        Ok(*self.checkpoint.lock().await)
    }

    async fn save(
        &self,
        checkpoint: CredentialRecoveryCheckpoint,
    ) -> Result<(), CredentialRecoveryCheckpointStoreError> {
        *self.checkpoint.lock().await = checkpoint;
        Ok(())
    }
}

fn current_position(events: &[StreamEvent], stream_id: &str) -> Option<StreamPosition> {
    events
        .iter()
        .filter(|event| event.stream_id() == stream_id)
        .map(|event| event.stream_position)
        .max()
}

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).unwrap()
}

fn app(
    events: ManagementTestStreamStore,
    secrets: MockOpenBaoSecretStore,
    registry: RuntimeCredentialRegistry,
) -> Router {
    test_router(events, secrets, registry, SecretString::new("admin-token").unwrap())
}

fn recovery_app(checkpoints: ManagementTestRecoveryCheckpointStore) -> Router {
    recovery_status_router(SecretString::new("admin-token").unwrap(), checkpoints)
}

fn request(method: Method, uri: &str, body: &str, token: Option<&str>) -> Request<Body> {
    request_with_idempotency(method, uri, body, token, token.map(|_| "idem-1"))
}

fn request_with_idempotency(
    method: Method,
    uri: &str,
    body: &str,
    token: Option<&str>,
    idempotency_key: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header(ADMIN_TOKEN_HEADER, token);
    }
    if let Some(idempotency_key) = idempotency_key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn runtime_key(source: SourceKind, integration_id: &str) -> RuntimeIntegrationKey {
    RuntimeIntegrationKey::new(source, &SourceIntegrationId::new(integration_id).unwrap())
}

fn source_runtime_key(source: SourceKind) -> RuntimeIntegrationKey {
    RuntimeIntegrationKey::for_source(source)
}

async fn resolved_plaintext(
    registry: &RuntimeCredentialRegistry,
    secrets: MockOpenBaoSecretStore,
) -> Result<String, RuntimeCredentialError> {
    resolved_target_plaintext(
        registry,
        secrets,
        SourceKind::GitHub,
        "primary",
        CredentialKind::WebhookSecret,
    )
    .await
}

async fn resolved_target_plaintext(
    registry: &RuntimeCredentialRegistry,
    secrets: MockOpenBaoSecretStore,
    source: SourceKind,
    integration_id: &str,
    kind: CredentialKind,
) -> Result<String, RuntimeCredentialError> {
    let material = registry
        .resolver(secrets)
        .resolve(&runtime_key(source, integration_id), kind)
        .await?;
    Ok(material.as_plaintext().unwrap().as_str().to_string())
}

async fn resolved_source_plaintext(
    registry: &RuntimeCredentialRegistry,
    secrets: MockOpenBaoSecretStore,
    source: SourceKind,
    kind: CredentialKind,
) -> Result<String, RuntimeCredentialError> {
    let material = registry
        .resolver(secrets)
        .resolve(&source_runtime_key(source), kind)
        .await?;
    Ok(material.as_plaintext().unwrap().as_str().to_string())
}

#[tokio::test]
async fn recovery_status_rejects_missing_admin_token() {
    let app = recovery_app(ManagementTestRecoveryCheckpointStore::default());

    let response = app
        .oneshot(request(Method::GET, "/recovery/status", "", None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn recovery_status_returns_metadata_only_checkpoint_state() {
    let checkpoints = ManagementTestRecoveryCheckpointStore::with_checkpoint(
        CredentialRecoveryCheckpoint::with_failure_state(41, 3, Some(0), Some(u64::MAX)),
    )
    .await;
    let app = recovery_app(checkpoints);

    let response = app
        .oneshot(request(Method::GET, "/recovery/status", "", Some("admin-token")))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["last_scanned_sequence"], 41);
    assert_eq!(body["next_scan_sequence"], 42);
    assert_eq!(body["consecutive_failure_count"], 3);
    assert_eq!(body["first_failure_unix_seconds"], 0);
    assert_eq!(body["retry_after_unix_seconds"], u64::MAX);
    assert_eq!(body["retry_delayed"], true);
    assert_eq!(body["stuck_recovery"], true);
    assert!(!body.to_string().contains("secret"));
}

#[tokio::test]
async fn put_rejects_missing_admin_token_before_mutating_state() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);

    let response = app
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(events.decoded_events().await.is_empty());
}

#[tokio::test]
async fn put_rejects_missing_idempotency_key_before_mutating_state() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);

    let response = app
        .oneshot(request_with_idempotency(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"], "missing idempotency key");
    assert!(events.decoded_events().await.is_empty());
}

#[tokio::test]
async fn put_writes_through_credential_handler_without_echoing_plaintext() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let response = app
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["state"], "active");
    assert_eq!(body["stream_position"], 2);
    assert_eq!(body["credential_ref"]["id"], "tenant-1:github/primary:webhook_secret");
    assert_eq!(body["credential_ref"]["version"], 1);
    assert!(!body.to_string().contains("super-secret"));
    assert_eq!(resolved_plaintext(&registry, secrets).await.unwrap(), "super-secret");
    assert_eq!(events.decoded_events().await.len(), 2);
}

#[tokio::test]
async fn responses_never_name_the_storage_backend() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events, secrets, registry);

    let put_body = response_json(
        app.clone()
            .oneshot(request(
                Method::PUT,
                "/github/primary/webhook-secret",
                r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
                Some("admin-token"),
            ))
            .await
            .unwrap(),
    )
    .await;
    let revoke_body = response_json(
        app.oneshot(request(
            Method::DELETE,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","version":1}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap(),
    )
    .await;

    for body in [&put_body, &revoke_body] {
        assert!(!body.to_string().contains("openbao"), "leaked storage backend: {body}");
    }
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:github/primary:webhook_secret"
    );
    assert_eq!(
        revoke_body["credential_ref"]["id"],
        "tenant-1:github/primary:webhook_secret"
    );
}

#[tokio::test]
async fn discord_bot_token_uses_source_scoped_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/discord/bot-token",
            r#"{"owner_id":"tenant-1","secret":"Bot old-token"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(put_body["credential_ref"]["id"], "tenant-1:discord:bot_token");
    assert_eq!(put_body["credential_ref"]["source"], "discord");
    assert_eq!(put_body["credential_ref"]["scope_key"], "discord");
    assert_eq!(put_body["credential_ref"]["kind"], "bot_token");
    assert!(!put_body.to_string().contains("Bot old-token"));
    assert_eq!(
        resolved_source_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Discord,
            CredentialKind::BotToken
        )
        .await
        .unwrap(),
        "Bot old-token"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/discord/bot-token/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"Bot new-token"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("Bot new-token"));
    assert_eq!(
        resolved_source_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Discord,
            CredentialKind::BotToken
        )
        .await
        .unwrap(),
        "Bot new-token"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/discord/bot-token",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_source_plaintext(&registry, secrets, SourceKind::Discord, CredentialKind::BotToken).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn slack_signing_secret_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/slack/primary/signing-secret",
            r#"{"owner_id":"tenant-1","secret":"old-slack-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:slack/primary:signing_secret"
    );
    assert_eq!(put_body["credential_ref"]["source"], "slack");
    assert_eq!(put_body["credential_ref"]["kind"], "signing_secret");
    assert!(!put_body.to_string().contains("old-slack-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Slack,
            "primary",
            CredentialKind::SigningSecret,
        )
        .await
        .unwrap(),
        "old-slack-secret"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/slack/primary/signing-secret/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-slack-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("new-slack-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Slack,
            "primary",
            CredentialKind::SigningSecret,
        )
        .await
        .unwrap(),
        "new-slack-secret"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/slack/primary/signing-secret",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Slack,
            "primary",
            CredentialKind::SigningSecret,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn gitlab_signing_token_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/gitlab/primary/signing-token",
            r#"{"owner_id":"tenant-1","secret":"whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:gitlab/primary:signing_token"
    );
    assert_eq!(put_body["credential_ref"]["source"], "gitlab");
    assert_eq!(put_body["credential_ref"]["kind"], "signing_token");
    assert!(
        !put_body
            .to_string()
            .contains("whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE=")
    );
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Gitlab,
            "primary",
            CredentialKind::SigningToken,
        )
        .await
        .unwrap(),
        "whsec_MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDE="
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/gitlab/primary/signing-token/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"whsec_YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY="}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(
        !rotate_body
            .to_string()
            .contains("whsec_YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY=")
    );
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Gitlab,
            "primary",
            CredentialKind::SigningToken,
        )
        .await
        .unwrap(),
        "whsec_YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY="
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/gitlab/primary/signing-token",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Gitlab,
            "primary",
            CredentialKind::SigningToken,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn incidentio_signing_secret_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/incidentio/primary/signing-secret",
            r#"{"owner_id":"tenant-1","secret":"whsec_b2xkLXNlY3JldA=="}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:incidentio/primary:signing_secret"
    );
    assert_eq!(put_body["credential_ref"]["source"], "incidentio");
    assert_eq!(put_body["credential_ref"]["kind"], "signing_secret");
    assert!(!put_body.to_string().contains("whsec_b2xkLXNlY3JldA=="));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Incidentio,
            "primary",
            CredentialKind::SigningSecret,
        )
        .await
        .unwrap(),
        "whsec_b2xkLXNlY3JldA=="
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/incidentio/primary/signing-secret/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"whsec_bmV3LXNlY3JldA=="}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("whsec_bmV3LXNlY3JldA=="));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Incidentio,
            "primary",
            CredentialKind::SigningSecret,
        )
        .await
        .unwrap(),
        "whsec_bmV3LXNlY3JldA=="
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/incidentio/primary/signing-secret",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Incidentio,
            "primary",
            CredentialKind::SigningSecret,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn linear_webhook_secret_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/linear/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"old-linear-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:linear/primary:webhook_secret"
    );
    assert_eq!(put_body["credential_ref"]["source"], "linear");
    assert_eq!(put_body["credential_ref"]["kind"], "webhook_secret");
    assert!(!put_body.to_string().contains("old-linear-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Linear,
            "primary",
            CredentialKind::WebhookSecret,
        )
        .await
        .unwrap(),
        "old-linear-secret"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/linear/primary/webhook-secret/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-linear-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("new-linear-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Linear,
            "primary",
            CredentialKind::WebhookSecret,
        )
        .await
        .unwrap(),
        "new-linear-secret"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/linear/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Linear,
            "primary",
            CredentialKind::WebhookSecret,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn microsoft_graph_client_state_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/microsoft-graph/primary/client-state",
            r#"{"owner_id":"tenant-1","secret":"old-microsoft-graph-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:microsoft_graph/primary:client_state"
    );
    assert_eq!(put_body["credential_ref"]["source"], "microsoft_graph");
    assert_eq!(put_body["credential_ref"]["kind"], "client_state");
    assert!(!put_body.to_string().contains("old-microsoft-graph-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::MicrosoftGraph,
            "primary",
            CredentialKind::ClientState,
        )
        .await
        .unwrap(),
        "old-microsoft-graph-secret"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/microsoft-graph/primary/client-state/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-microsoft-graph-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("new-microsoft-graph-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::MicrosoftGraph,
            "primary",
            CredentialKind::ClientState,
        )
        .await
        .unwrap(),
        "new-microsoft-graph-secret"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/microsoft-graph/primary/client-state",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::MicrosoftGraph,
            "primary",
            CredentialKind::ClientState,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn sentry_client_secret_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/sentry/primary/client-secret",
            r#"{"owner_id":"tenant-1","secret":"old-sentry-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:sentry/primary:client_secret"
    );
    assert_eq!(put_body["credential_ref"]["source"], "sentry");
    assert_eq!(put_body["credential_ref"]["kind"], "client_secret");
    assert!(!put_body.to_string().contains("old-sentry-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Sentry,
            "primary",
            CredentialKind::ClientSecret,
        )
        .await
        .unwrap(),
        "old-sentry-secret"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/sentry/primary/client-secret/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-sentry-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("new-sentry-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Sentry,
            "primary",
            CredentialKind::ClientSecret,
        )
        .await
        .unwrap(),
        "new-sentry-secret"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/sentry/primary/client-secret",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Sentry,
            "primary",
            CredentialKind::ClientSecret,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn notion_verification_token_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/notion/primary/verification-token",
            r#"{"owner_id":"tenant-1","secret":"old-notion-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:notion/primary:verification_token"
    );
    assert_eq!(put_body["credential_ref"]["source"], "notion");
    assert_eq!(put_body["credential_ref"]["kind"], "verification_token");
    assert!(!put_body.to_string().contains("old-notion-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Notion,
            "primary",
            CredentialKind::VerificationToken,
        )
        .await
        .unwrap(),
        "old-notion-secret"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/notion/primary/verification-token/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-notion-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("new-notion-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Notion,
            "primary",
            CredentialKind::VerificationToken,
        )
        .await
        .unwrap(),
        "new-notion-secret"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/notion/primary/verification-token",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Notion,
            "primary",
            CredentialKind::VerificationToken,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn telegram_webhook_secret_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/telegram/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"old-telegram-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:telegram/primary:webhook_secret"
    );
    assert_eq!(put_body["credential_ref"]["source"], "telegram");
    assert_eq!(put_body["credential_ref"]["kind"], "webhook_secret");
    assert!(!put_body.to_string().contains("old-telegram-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Telegram,
            "primary",
            CredentialKind::WebhookSecret,
        )
        .await
        .unwrap(),
        "old-telegram-secret"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/telegram/primary/webhook-secret/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-telegram-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("new-telegram-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Telegram,
            "primary",
            CredentialKind::WebhookSecret,
        )
        .await
        .unwrap(),
        "new-telegram-secret"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/telegram/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Telegram,
            "primary",
            CredentialKind::WebhookSecret,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn twitter_consumer_secret_uses_credential_handler_for_put_rotate_and_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    let put_response = app
        .clone()
        .oneshot(request(
            Method::PUT,
            "/twitter/primary/consumer-secret",
            r#"{"owner_id":"tenant-1","secret":"old-twitter-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(put_response.status(), StatusCode::OK);
    let put_body = response_json(put_response).await;
    assert_eq!(put_body["state"], "active");
    assert_eq!(
        put_body["credential_ref"]["id"],
        "tenant-1:twitter/primary:consumer_secret"
    );
    assert_eq!(put_body["credential_ref"]["source"], "twitter");
    assert_eq!(put_body["credential_ref"]["kind"], "consumer_secret");
    assert!(!put_body.to_string().contains("old-twitter-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Twitter,
            "primary",
            CredentialKind::ConsumerSecret,
        )
        .await
        .unwrap(),
        "old-twitter-secret"
    );

    let rotate_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/twitter/primary/consumer-secret/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-twitter-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotate_body = response_json(rotate_response).await;
    assert_eq!(rotate_body["state"], "active");
    assert_eq!(rotate_body["credential_ref"]["version"], 2);
    assert!(!rotate_body.to_string().contains("new-twitter-secret"));
    assert_eq!(
        resolved_target_plaintext(
            &registry,
            secrets.clone(),
            SourceKind::Twitter,
            "primary",
            CredentialKind::ConsumerSecret,
        )
        .await
        .unwrap(),
        "new-twitter-secret"
    );

    let revoke_response = app
        .oneshot(request(
            Method::DELETE,
            "/twitter/primary/consumer-secret",
            r#"{"owner_id":"tenant-1","version":2}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(revoke_response.status(), StatusCode::OK);
    let revoke_body = response_json(revoke_response).await;
    assert_eq!(revoke_body["state"], "revoked");
    assert_eq!(revoke_body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_target_plaintext(
            &registry,
            secrets,
            SourceKind::Twitter,
            "primary",
            CredentialKind::ConsumerSecret,
        )
        .await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn put_replay_returns_same_metadata_snapshot_without_new_credential_events() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);
    let body = r#"{"owner_id":"tenant-1","secret":"super-secret"}"#;

    let first = app
        .clone()
        .oneshot(request_with_idempotency(
            Method::PUT,
            "/github/primary/webhook-secret",
            body,
            Some("admin-token"),
            Some("same-logical-action"),
        ))
        .await
        .unwrap();
    let first_status = first.status();
    let first_body = response_json(first).await;
    let second = app
        .oneshot(request_with_idempotency(
            Method::PUT,
            "/github/primary/webhook-secret",
            body,
            Some("admin-token"),
            Some("same-logical-action"),
        ))
        .await
        .unwrap();
    let second_status = second.status();
    let second_body = response_json(second).await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_body, first_body);
    assert!(!second_body.to_string().contains("super-secret"));
    assert_eq!(events.decoded_events().await.len(), 2);
}

#[tokio::test]
async fn put_rejects_same_idempotency_scope_with_different_fingerprint() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    app.clone()
        .oneshot(request_with_idempotency(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"first-secret"}"#,
            Some("admin-token"),
            Some("conflicting-action"),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(request_with_idempotency(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"second-secret"}"#,
            Some("admin-token"),
            Some("conflicting-action"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response_json(response).await;
    assert_eq!(body["error"], "idempotency conflict");
    assert_eq!(resolved_plaintext(&registry, secrets).await.unwrap(), "first-secret");
    assert_eq!(events.decoded_events().await.len(), 2);
}

#[tokio::test]
async fn same_raw_idempotency_key_is_scoped_by_owner_and_target() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);

    let first = app
        .clone()
        .oneshot(request_with_idempotency(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"tenant-one-secret"}"#,
            Some("admin-token"),
            Some("shared-client-key"),
        ))
        .await
        .unwrap();
    let second = app
        .oneshot(request_with_idempotency(
            Method::PUT,
            "/github/secondary/webhook-secret",
            r#"{"owner_id":"tenant-2","secret":"tenant-two-secret"}"#,
            Some("admin-token"),
            Some("shared-client-key"),
        ))
        .await
        .unwrap();

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = response_json(second).await;
    assert_eq!(second_body["credential_ref"]["owner_id"], "tenant-2");
    assert!(!second_body.to_string().contains("tenant-two-secret"));
    assert_eq!(events.decoded_events().await.len(), 4);
}

#[tokio::test]
async fn rotate_updates_runtime_projection_without_plaintext_response() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    app.clone()
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"old-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(request(
            Method::POST,
            "/github/primary/webhook-secret/rotations",
            r#"{"owner_id":"tenant-1","version":1,"secret":"new-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["state"], "active");
    assert_eq!(body["credential_ref"]["version"], 2);
    assert!(!body.to_string().contains("new-secret"));
    assert_eq!(resolved_plaintext(&registry, secrets).await.unwrap(), "new-secret");
    assert_eq!(events.decoded_events().await.len(), 4);
}

#[tokio::test]
async fn revoke_removes_runtime_projection() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    app.clone()
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(request(
            Method::DELETE,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","version":1}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["state"], "revoked");
    assert_eq!(body["credential_ref"]["status"], "revoked");
    assert!(matches!(
        resolved_plaintext(&registry, secrets).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 3);
}

#[tokio::test]
async fn destroy_completes_lifecycle_after_revoke() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    app.clone()
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(request(
            Method::DELETE,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","version":1}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(request(
            Method::POST,
            "/github/primary/webhook-secret/destructions",
            r#"{"owner_id":"tenant-1","version":1,"reason":"credential lifecycle cleanup"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["state"], "destroyed");
    assert_eq!(body["credential_ref"]["status"], "destroyed");
    assert!(matches!(
        resolved_plaintext(&registry, secrets).await,
        Err(RuntimeCredentialError::IntegrationNotFound { .. })
    ));
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn destroy_rejects_missing_admin_token_before_mutating_state() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);

    let response = app
        .oneshot(request(
            Method::POST,
            "/github/primary/webhook-secret/destructions",
            r#"{"owner_id":"tenant-1","version":1,"reason":"credential lifecycle cleanup"}"#,
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(events.decoded_events().await.is_empty());
}

#[tokio::test]
async fn destroy_rejects_active_credential_with_conflict() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);

    app.clone()
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(request(
            Method::POST,
            "/github/primary/webhook-secret/destructions",
            r#"{"owner_id":"tenant-1","version":1,"reason":"credential lifecycle cleanup"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response_json(response).await;
    assert_eq!(body["error"], "credential destroy request rejected");
    assert_eq!(events.decoded_events().await.len(), 2);
}

#[tokio::test]
async fn destroy_replay_returns_same_response_without_new_credential_events() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);

    app.clone()
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(request(
            Method::DELETE,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","version":1}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    let body = r#"{"owner_id":"tenant-1","version":1,"reason":"credential lifecycle cleanup"}"#;
    let first = app
        .clone()
        .oneshot(request_with_idempotency(
            Method::POST,
            "/github/primary/webhook-secret/destructions",
            body,
            Some("admin-token"),
            Some("same-logical-action"),
        ))
        .await
        .unwrap();
    let first_status = first.status();
    let first_body = response_json(first).await;
    let second = app
        .oneshot(request_with_idempotency(
            Method::POST,
            "/github/primary/webhook-secret/destructions",
            body,
            Some("admin-token"),
            Some("same-logical-action"),
        ))
        .await
        .unwrap();
    let second_status = second.status();
    let second_body = response_json(second).await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_body, first_body);
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn destroy_rejects_same_idempotency_scope_with_different_fingerprint() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets.clone(), registry.clone());

    app.clone()
        .oneshot(request(
            Method::PUT,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(request(
            Method::DELETE,
            "/github/primary/webhook-secret",
            r#"{"owner_id":"tenant-1","version":1}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(request_with_idempotency(
            Method::POST,
            "/github/primary/webhook-secret/destructions",
            r#"{"owner_id":"tenant-1","version":1,"reason":"first reason"}"#,
            Some("admin-token"),
            Some("conflicting-destroy"),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(request_with_idempotency(
            Method::POST,
            "/github/primary/webhook-secret/destructions",
            r#"{"owner_id":"tenant-1","version":1,"reason":"second reason"}"#,
            Some("admin-token"),
            Some("conflicting-destroy"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response_json(response).await;
    assert_eq!(body["error"], "idempotency conflict");
    assert_eq!(events.decoded_events().await.len(), 5);
}

#[tokio::test]
async fn invalid_input_is_rejected_without_recording_credential_events() {
    let events = ManagementTestStreamStore::default();
    let secrets = MockOpenBaoSecretStore::default();
    let registry = RuntimeCredentialRegistry::default();
    let app = app(events.clone(), secrets, registry);

    let response = app
        .oneshot(request(
            Method::PUT,
            "/github/not.valid/webhook-secret",
            r#"{"owner_id":"tenant-1","secret":"super-secret"}"#,
            Some("admin-token"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(events.decoded_events().await.is_empty());
}
