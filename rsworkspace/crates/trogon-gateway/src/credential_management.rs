use std::error::Error;
use std::fmt;
use std::time::SystemTime;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tracing::warn;
use trogon_decider_runtime::{SnapshotRead, SnapshotWrite, StreamAppend, StreamPosition, StreamRead};
use trogon_std::SecretString;
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1};

use crate::credential::commands::domain::{
    CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, CredentialStatus, CredentialVersion, SourceKind,
};
use crate::credential::handler::{
    CredentialRuntimeHandler, CredentialRuntimeHandlerError, PutCredential, RevokeStoredCredential, RotateCredential,
};
use crate::credential::processor::recovery_worker::{CredentialRecoveryCheckpointStore, CredentialRecoveryPolicy};
use crate::credential::processor::runtime_projection::RuntimeCredentialRegistry;
use crate::credential::proto::{decode_active_state, decode_message_field, decode_revoked_state};
#[cfg(test)]
use crate::credential_management_idempotency::CredentialCommandInMemoryIdempotencyLedger;
use crate::credential_management_idempotency::{
    CredentialCommandIdempotencyStore, CredentialCommandIdempotencyStoreError, IdempotencyDecision, IdempotencyKey,
    IdempotencyScope, RequestFingerprint,
};
use crate::secret_store::openbao_secret_store::openbao_credential_id;
use crate::secret_store::{
    SecretStoreError, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
};
use crate::source_integration_id::SourceIntegrationId;

pub(crate) const ADMIN_TOKEN_HEADER: &str = "x-trogon-admin-token";
pub(crate) const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct CredentialManagementState<EventStore, Secrets, Idempotency> {
    handler: CredentialRuntimeHandler<EventStore, Secrets>,
    admin_token: SecretString,
    idempotency: Idempotency,
}

#[derive(Clone)]
struct CredentialRecoveryStatusState<Checkpoints> {
    admin_token: SecretString,
    checkpoints: Checkpoints,
}

#[derive(Clone, Copy)]
struct ManagedCredentialTarget {
    source: SourceKind,
    kind: CredentialKind,
}

const GITHUB_WEBHOOK_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::GitHub,
    kind: CredentialKind::WebhookSecret,
};

const DISCORD_BOT_TOKEN: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Discord,
    kind: CredentialKind::BotToken,
};

const GITLAB_SIGNING_TOKEN: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Gitlab,
    kind: CredentialKind::SigningToken,
};

const INCIDENTIO_SIGNING_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Incidentio,
    kind: CredentialKind::SigningSecret,
};

const LINEAR_WEBHOOK_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Linear,
    kind: CredentialKind::WebhookSecret,
};

const MICROSOFT_GRAPH_CLIENT_STATE: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::MicrosoftGraph,
    kind: CredentialKind::ClientState,
};

const NOTION_VERIFICATION_TOKEN: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Notion,
    kind: CredentialKind::VerificationToken,
};

const SENTRY_CLIENT_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Sentry,
    kind: CredentialKind::ClientSecret,
};

const SLACK_SIGNING_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Slack,
    kind: CredentialKind::SigningSecret,
};

const TELEGRAM_WEBHOOK_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Telegram,
    kind: CredentialKind::WebhookSecret,
};

const TWITTER_CONSUMER_SECRET: ManagedCredentialTarget = ManagedCredentialTarget {
    source: SourceKind::Twitter,
    kind: CredentialKind::ConsumerSecret,
};

pub(crate) fn router<EventStore, Secrets, Idempotency>(
    event_store: EventStore,
    secrets: Secrets,
    runtime_credentials: RuntimeCredentialRegistry,
    admin_token: SecretString,
    idempotency: Idempotency,
) -> Router
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError>
        + SecretStoreRotate<Error = SecretStoreError>
        + SecretStoreRevoke<Error = SecretStoreError>
        + SecretStoreMetadata<Error = SecretStoreError>
        + Clone
        + Send
        + Sync
        + 'static,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let state = CredentialManagementState {
        handler: CredentialRuntimeHandler::new(event_store, secrets, runtime_credentials),
        admin_token,
        idempotency,
    };

    Router::new()
        .route(
            "/discord/bot-token",
            put(put_discord_bot_token::<EventStore, Secrets, Idempotency>)
                .delete(revoke_discord_bot_token::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/discord/bot-token/rotations",
            post(rotate_discord_bot_token::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/github/{integration_id}/webhook-secret",
            put(put_github_webhook_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_github_webhook_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/github/{integration_id}/webhook-secret/rotations",
            post(rotate_github_webhook_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/gitlab/{integration_id}/signing-token",
            put(put_gitlab_signing_token::<EventStore, Secrets, Idempotency>)
                .delete(revoke_gitlab_signing_token::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/gitlab/{integration_id}/signing-token/rotations",
            post(rotate_gitlab_signing_token::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/incidentio/{integration_id}/signing-secret",
            put(put_incidentio_signing_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_incidentio_signing_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/incidentio/{integration_id}/signing-secret/rotations",
            post(rotate_incidentio_signing_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/linear/{integration_id}/webhook-secret",
            put(put_linear_webhook_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_linear_webhook_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/linear/{integration_id}/webhook-secret/rotations",
            post(rotate_linear_webhook_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/microsoft-graph/{integration_id}/client-state",
            put(put_microsoft_graph_client_state::<EventStore, Secrets, Idempotency>)
                .delete(revoke_microsoft_graph_client_state::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/microsoft-graph/{integration_id}/client-state/rotations",
            post(rotate_microsoft_graph_client_state::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/notion/{integration_id}/verification-token",
            put(put_notion_verification_token::<EventStore, Secrets, Idempotency>)
                .delete(revoke_notion_verification_token::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/notion/{integration_id}/verification-token/rotations",
            post(rotate_notion_verification_token::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/sentry/{integration_id}/client-secret",
            put(put_sentry_client_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_sentry_client_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/sentry/{integration_id}/client-secret/rotations",
            post(rotate_sentry_client_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/slack/{integration_id}/signing-secret",
            put(put_slack_signing_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_slack_signing_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/slack/{integration_id}/signing-secret/rotations",
            post(rotate_slack_signing_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/telegram/{integration_id}/webhook-secret",
            put(put_telegram_webhook_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_telegram_webhook_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/telegram/{integration_id}/webhook-secret/rotations",
            post(rotate_telegram_webhook_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/twitter/{integration_id}/consumer-secret",
            put(put_twitter_consumer_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_twitter_consumer_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/twitter/{integration_id}/consumer-secret/rotations",
            post(rotate_twitter_consumer_secret::<EventStore, Secrets, Idempotency>),
        )
        .with_state(state)
}

pub(crate) fn recovery_status_router<Checkpoints>(admin_token: SecretString, checkpoints: Checkpoints) -> Router
where
    Checkpoints: CredentialRecoveryCheckpointStore,
{
    Router::new()
        .route("/recovery/status", get(recovery_status::<Checkpoints>))
        .with_state(CredentialRecoveryStatusState {
            admin_token,
            checkpoints,
        })
}

#[cfg(test)]
fn test_router<EventStore, Secrets>(
    event_store: EventStore,
    secrets: Secrets,
    runtime_credentials: RuntimeCredentialRegistry,
    admin_token: SecretString,
) -> Router
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError>
        + SecretStoreRotate<Error = SecretStoreError>
        + SecretStoreRevoke<Error = SecretStoreError>
        + SecretStoreMetadata<Error = SecretStoreError>
        + Clone
        + Send
        + Sync
        + 'static,
{
    router(
        event_store,
        secrets,
        runtime_credentials,
        admin_token,
        CredentialCommandInMemoryIdempotencyLedger::default(),
    )
}

async fn recovery_status<Checkpoints>(
    State(state): State<CredentialRecoveryStatusState<Checkpoints>>,
    headers: HeaderMap,
) -> Result<Json<CredentialRecoveryStatusResponse>, CredentialManagementHttpError>
where
    Checkpoints: CredentialRecoveryCheckpointStore,
{
    authorize(&headers, &state.admin_token)?;
    let checkpoint = state
        .checkpoints
        .load()
        .await
        .map_err(CredentialManagementHttpError::recovery_checkpoint)?;
    let now = SystemTime::now();
    let policy = CredentialRecoveryPolicy::default();
    Ok(Json(CredentialRecoveryStatusResponse {
        last_scanned_sequence: checkpoint.last_scanned_sequence(),
        next_scan_sequence: checkpoint.next_sequence(),
        consecutive_failure_count: checkpoint.consecutive_failure_count(),
        first_failure_unix_seconds: checkpoint.first_failure_unix_seconds(),
        retry_after_unix_seconds: checkpoint.retry_after_unix_seconds(),
        retry_delayed: checkpoint.retry_delayed_at(now),
        stuck_recovery: checkpoint.stuck_at(now, policy),
    }))
}

async fn put_discord_bot_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_source_credential_secret(state, headers, request, DISCORD_BOT_TOKEN).await
}

async fn put_github_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, GITHUB_WEBHOOK_SECRET).await
}

async fn put_gitlab_signing_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, GITLAB_SIGNING_TOKEN).await
}

async fn put_incidentio_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, INCIDENTIO_SIGNING_SECRET).await
}

async fn put_slack_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, SLACK_SIGNING_SECRET).await
}

async fn put_linear_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, LINEAR_WEBHOOK_SECRET).await
}

async fn put_microsoft_graph_client_state<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, MICROSOFT_GRAPH_CLIENT_STATE).await
}

async fn put_sentry_client_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, SENTRY_CLIENT_SECRET).await
}

async fn put_notion_verification_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, NOTION_VERIFICATION_TOKEN).await
}

async fn put_telegram_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, TELEGRAM_WEBHOOK_SECRET).await
}

async fn put_twitter_consumer_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<WriteCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    put_credential_secret(state, path, headers, request, TWITTER_CONSUMER_SECRET).await
}

async fn put_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    path: ManagedCredentialPath,
    headers: HeaderMap,
    request: WriteCredentialSecretRequest,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = integration_credential_scope(request.owner_id, path.integration_id, target.source)?;
    put_scoped_credential_secret(state, scope, headers, request.secret, target).await
}

async fn put_source_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    headers: HeaderMap,
    request: WriteCredentialSecretRequest,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = source_credential_scope(request.owner_id, target.source)?;
    put_scoped_credential_secret(state, scope, headers, request.secret, target).await
}

async fn put_scoped_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    scope: CredentialScope,
    headers: HeaderMap,
    secret_value: String,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStorePut<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    authorize(&headers, &state.admin_token)?;
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    let credential_id =
        openbao_credential_id(&scope, target.kind).map_err(CredentialManagementHttpError::invalid_input)?;
    let scope_key = scope.scope_key();
    let fingerprint = request_fingerprint(
        &state.admin_token,
        &[
            CommandNamespace::Create.as_str(),
            scope.owner_id().as_str(),
            scope_key.as_str(),
            credential_id.as_str(),
            target.kind.as_str(),
            secret_value.as_str(),
        ],
    );
    let idempotency_scope = IdempotencyScope::new(
        scope.owner_id().as_str(),
        CommandNamespace::Create.as_str(),
        credential_id.as_str(),
        idempotency_key,
    );
    match state
        .idempotency
        .begin(idempotency_scope.clone(), fingerprint)
        .await
        .map_err(CredentialManagementHttpError::idempotency_store)?
    {
        IdempotencyDecision::Replay(response) => return Ok(Json(response)),
        IdempotencyDecision::Execute => {}
        IdempotencyDecision::Conflict => return Err(CredentialManagementHttpError::IdempotencyConflict),
        IdempotencyDecision::InProgress => return Err(CredentialManagementHttpError::IdempotencyInProgress),
    }
    let secret = SecretString::new(secret_value).map_err(CredentialManagementHttpError::invalid_input)?;
    let command = PutCredential::new(credential_id, scope, target.kind, secret);

    let outcome = match state.handler.put(command).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(CredentialManagementHttpError::command_failed(error));
        }
    };

    let response = match command_response(outcome.stream_position(), outcome.into_state()) {
        Ok(response) => response,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(error);
        }
    };
    state
        .idempotency
        .complete(&idempotency_scope, response.clone())
        .await
        .map_err(CredentialManagementHttpError::idempotency_store)?;
    Ok(Json(response))
}

async fn rotate_discord_bot_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_source_credential_secret(state, headers, request, DISCORD_BOT_TOKEN).await
}

async fn rotate_github_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, GITHUB_WEBHOOK_SECRET).await
}

async fn rotate_gitlab_signing_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, GITLAB_SIGNING_TOKEN).await
}

async fn rotate_incidentio_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, INCIDENTIO_SIGNING_SECRET).await
}

async fn rotate_slack_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, SLACK_SIGNING_SECRET).await
}

async fn rotate_linear_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, LINEAR_WEBHOOK_SECRET).await
}

async fn rotate_microsoft_graph_client_state<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, MICROSOFT_GRAPH_CLIENT_STATE).await
}

async fn rotate_sentry_client_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, SENTRY_CLIENT_SECRET).await
}

async fn rotate_notion_verification_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, NOTION_VERIFICATION_TOKEN).await
}

async fn rotate_telegram_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, TELEGRAM_WEBHOOK_SECRET).await
}

async fn rotate_twitter_consumer_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RotateCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    rotate_credential_secret(state, path, headers, request, TWITTER_CONSUMER_SECRET).await
}

async fn rotate_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    path: ManagedCredentialPath,
    headers: HeaderMap,
    request: RotateCredentialSecretRequest,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = integration_credential_scope(request.owner_id, path.integration_id, target.source)?;
    rotate_scoped_credential_secret(state, scope, headers, request.version, request.secret, target).await
}

async fn rotate_source_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    headers: HeaderMap,
    request: RotateCredentialSecretRequest,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = source_credential_scope(request.owner_id, target.source)?;
    rotate_scoped_credential_secret(state, scope, headers, request.version, request.secret, target).await
}

async fn rotate_scoped_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    scope: CredentialScope,
    headers: HeaderMap,
    request_version: u64,
    secret_value: String,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    authorize(&headers, &state.admin_token)?;
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    let credential_ref = credential_ref_from_request(&scope, target.kind, request_version)?;
    let scope_key = scope.scope_key();
    let version = request_version.to_string();
    let fingerprint = request_fingerprint(
        &state.admin_token,
        &[
            CommandNamespace::Rotate.as_str(),
            scope.owner_id().as_str(),
            scope_key.as_str(),
            credential_ref.id().as_str(),
            version.as_str(),
            target.kind.as_str(),
            secret_value.as_str(),
        ],
    );
    let idempotency_scope = IdempotencyScope::new(
        scope.owner_id().as_str(),
        CommandNamespace::Rotate.as_str(),
        credential_ref.id().as_str(),
        idempotency_key,
    );
    match state
        .idempotency
        .begin(idempotency_scope.clone(), fingerprint)
        .await
        .map_err(CredentialManagementHttpError::idempotency_store)?
    {
        IdempotencyDecision::Replay(response) => return Ok(Json(response)),
        IdempotencyDecision::Execute => {}
        IdempotencyDecision::Conflict => return Err(CredentialManagementHttpError::IdempotencyConflict),
        IdempotencyDecision::InProgress => return Err(CredentialManagementHttpError::IdempotencyInProgress),
    }
    let secret = SecretString::new(secret_value).map_err(CredentialManagementHttpError::invalid_input)?;
    let command = RotateCredential::new(credential_ref, secret);

    let outcome = match state.handler.rotate(command).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(CredentialManagementHttpError::command_failed(error));
        }
    };

    let response = match command_response(outcome.stream_position(), outcome.into_state()) {
        Ok(response) => response,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(error);
        }
    };
    state
        .idempotency
        .complete(&idempotency_scope, response.clone())
        .await
        .map_err(CredentialManagementHttpError::idempotency_store)?;
    Ok(Json(response))
}

async fn revoke_discord_bot_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_source_credential_secret(state, headers, request, DISCORD_BOT_TOKEN).await
}

async fn revoke_github_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, GITHUB_WEBHOOK_SECRET).await
}

async fn revoke_gitlab_signing_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, GITLAB_SIGNING_TOKEN).await
}

async fn revoke_incidentio_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, INCIDENTIO_SIGNING_SECRET).await
}

async fn revoke_slack_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, SLACK_SIGNING_SECRET).await
}

async fn revoke_linear_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, LINEAR_WEBHOOK_SECRET).await
}

async fn revoke_microsoft_graph_client_state<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, MICROSOFT_GRAPH_CLIENT_STATE).await
}

async fn revoke_sentry_client_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, SENTRY_CLIENT_SECRET).await
}

async fn revoke_notion_verification_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, NOTION_VERIFICATION_TOKEN).await
}

async fn revoke_telegram_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, TELEGRAM_WEBHOOK_SECRET).await
}

async fn revoke_twitter_consumer_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<RevokeCredentialSecretRequest>,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    revoke_credential_secret(state, path, headers, request, TWITTER_CONSUMER_SECRET).await
}

async fn revoke_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    path: ManagedCredentialPath,
    headers: HeaderMap,
    request: RevokeCredentialSecretRequest,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = integration_credential_scope(request.owner_id, path.integration_id, target.source)?;
    revoke_scoped_credential_secret(state, scope, headers, request.version, target).await
}

async fn revoke_source_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    headers: HeaderMap,
    request: RevokeCredentialSecretRequest,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = source_credential_scope(request.owner_id, target.source)?;
    revoke_scoped_credential_secret(state, scope, headers, request.version, target).await
}

async fn revoke_scoped_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    scope: CredentialScope,
    headers: HeaderMap,
    request_version: u64,
    target: ManagedCredentialTarget,
) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + Send
        + Sync
        + 'static,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    <EventStore as StreamAppend<str>>::Error: Error + Send + Sync + 'static,
    Secrets: SecretStoreRevoke<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    authorize(&headers, &state.admin_token)?;
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    let credential_ref = credential_ref_from_request(&scope, target.kind, request_version)?;
    let scope_key = scope.scope_key();
    let version = request_version.to_string();
    let fingerprint = request_fingerprint(
        &state.admin_token,
        &[
            CommandNamespace::Revoke.as_str(),
            scope.owner_id().as_str(),
            scope_key.as_str(),
            credential_ref.id().as_str(),
            version.as_str(),
            target.kind.as_str(),
        ],
    );
    let idempotency_scope = IdempotencyScope::new(
        scope.owner_id().as_str(),
        CommandNamespace::Revoke.as_str(),
        credential_ref.id().as_str(),
        idempotency_key,
    );
    match state
        .idempotency
        .begin(idempotency_scope.clone(), fingerprint)
        .await
        .map_err(CredentialManagementHttpError::idempotency_store)?
    {
        IdempotencyDecision::Replay(response) => return Ok(Json(response)),
        IdempotencyDecision::Execute => {}
        IdempotencyDecision::Conflict => return Err(CredentialManagementHttpError::IdempotencyConflict),
        IdempotencyDecision::InProgress => return Err(CredentialManagementHttpError::IdempotencyInProgress),
    }
    let command = RevokeStoredCredential::new(credential_ref);

    let outcome = match state.handler.revoke(command).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(CredentialManagementHttpError::command_failed(error));
        }
    };

    let response = match command_response(outcome.stream_position(), outcome.into_state()) {
        Ok(response) => response,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(error);
        }
    };
    state
        .idempotency
        .complete(&idempotency_scope, response.clone())
        .await
        .map_err(CredentialManagementHttpError::idempotency_store)?;
    Ok(Json(response))
}

impl<EventStore, Secrets, Idempotency> CredentialManagementState<EventStore, Secrets, Idempotency>
where
    Idempotency: CredentialCommandIdempotencyStore,
{
    async fn abandon_idempotency(&self, scope: &IdempotencyScope) {
        if let Err(error) = self.idempotency.abandon(scope).await {
            warn!(error = %error, "credential management idempotency abandon failed");
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CommandNamespace {
    Create,
    Rotate,
    Revoke,
}

impl CommandNamespace {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "credential.create",
            Self::Rotate => "credential.rotate",
            Self::Revoke => "credential.revoke",
        }
    }
}

#[derive(Deserialize)]
struct ManagedCredentialPath {
    integration_id: String,
}

#[derive(Deserialize)]
struct WriteCredentialSecretRequest {
    owner_id: String,
    secret: String,
}

#[derive(Deserialize)]
struct RotateCredentialSecretRequest {
    owner_id: String,
    version: u64,
    secret: String,
}

#[derive(Deserialize)]
struct RevokeCredentialSecretRequest {
    owner_id: String,
    version: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CredentialCommandResponse {
    pub(crate) state: String,
    pub(crate) stream_position: u64,
    pub(crate) credential_ref: CredentialRefResponse,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CredentialRefResponse {
    pub(crate) id: String,
    pub(crate) version: u64,
    pub(crate) owner_id: String,
    pub(crate) source: String,
    pub(crate) scope_key: String,
    pub(crate) kind: String,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CredentialRecoveryStatusResponse {
    pub(crate) last_scanned_sequence: u64,
    pub(crate) next_scan_sequence: u64,
    pub(crate) consecutive_failure_count: u32,
    pub(crate) first_failure_unix_seconds: Option<u64>,
    pub(crate) retry_after_unix_seconds: Option<u64>,
    pub(crate) retry_delayed: bool,
    pub(crate) stuck_recovery: bool,
}

#[derive(Debug, Serialize)]
struct CredentialManagementErrorResponse {
    error: &'static str,
}

#[derive(Debug)]
enum CredentialManagementHttpError {
    Unauthorized,
    MissingIdempotencyKey,
    IdempotencyConflict,
    IdempotencyInProgress,
    IdempotencyStore(String),
    RecoveryCheckpoint(String),
    InvalidInput(String),
    CommandFailed(String),
    UnexpectedCredentialState(&'static str),
    InvalidState(String),
}

impl CredentialManagementHttpError {
    fn invalid_input(error: impl fmt::Display) -> Self {
        Self::InvalidInput(error.to_string())
    }

    fn invalid_state(error: impl fmt::Display) -> Self {
        warn!(error = %error, "credential management persisted state is invalid");
        Self::InvalidState(error.to_string())
    }

    fn command_failed<SnapshotReadError, ReadError, AppendError>(
        error: CredentialRuntimeHandlerError<SecretStoreError, SnapshotReadError, ReadError, AppendError>,
    ) -> Self
    where
        SnapshotReadError: Error + Send + Sync + 'static,
        ReadError: Error + Send + Sync + 'static,
        AppendError: Error + Send + Sync + 'static,
    {
        warn!(error = %error, "credential management command failed");
        Self::CommandFailed(error.to_string())
    }

    fn idempotency_store(error: CredentialCommandIdempotencyStoreError) -> Self {
        warn!(error = %error, "credential management idempotency store failed");
        match error {
            CredentialCommandIdempotencyStoreError::Conflict => Self::IdempotencyInProgress,
            source => Self::IdempotencyStore(source.to_string()),
        }
    }

    fn recovery_checkpoint(error: impl fmt::Display) -> Self {
        warn!(error = %error, "credential recovery checkpoint status failed");
        Self::RecoveryCheckpoint(error.to_string())
    }
}

impl IntoResponse for CredentialManagementHttpError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::MissingIdempotencyKey => (StatusCode::BAD_REQUEST, "missing idempotency key"),
            Self::IdempotencyConflict => (StatusCode::CONFLICT, "idempotency conflict"),
            Self::IdempotencyInProgress => (StatusCode::CONFLICT, "idempotency request is already in progress"),
            Self::IdempotencyStore(error) => {
                warn!(error = %error, "credential management idempotency request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential management idempotency failed",
                )
            }
            Self::RecoveryCheckpoint(error) => {
                warn!(error = %error, "credential recovery checkpoint request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential recovery checkpoint failed",
                )
            }
            Self::InvalidInput(error) => {
                warn!(error = %error, "credential management input rejected");
                (StatusCode::BAD_REQUEST, "invalid credential management request")
            }
            Self::CommandFailed(error) => {
                warn!(error = %error, "credential management request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential management request failed",
                )
            }
            Self::UnexpectedCredentialState(state) => {
                warn!(state, "credential management command returned an unexpected state");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential management request failed",
                )
            }
            Self::InvalidState(error) => {
                warn!(error = %error, "credential management persisted state is invalid");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential management request failed",
                )
            }
        };

        (status, Json(CredentialManagementErrorResponse { error })).into_response()
    }
}

fn idempotency_key_from_headers(headers: &HeaderMap) -> Result<IdempotencyKey, CredentialManagementHttpError> {
    let Some(provided) = headers
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(CredentialManagementHttpError::MissingIdempotencyKey);
    };
    IdempotencyKey::new(provided).map_err(CredentialManagementHttpError::invalid_input)
}

fn request_fingerprint(admin_token: &SecretString, parts: &[&str]) -> RequestFingerprint {
    let mut mac = HmacSha256::new_from_slice(admin_token.as_str().as_bytes()).expect("HMAC accepts any key length");
    for part in parts {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part.as_bytes());
    }
    RequestFingerprint::new(hex::encode(mac.finalize().into_bytes()))
}

fn authorize(headers: &HeaderMap, admin_token: &SecretString) -> Result<(), CredentialManagementHttpError> {
    let Some(provided) = headers.get(ADMIN_TOKEN_HEADER).and_then(|value| value.to_str().ok()) else {
        return Err(CredentialManagementHttpError::Unauthorized);
    };
    if bool::from(provided.as_bytes().ct_eq(admin_token.as_str().as_bytes())) {
        Ok(())
    } else {
        Err(CredentialManagementHttpError::Unauthorized)
    }
}

fn integration_credential_scope(
    owner_id: String,
    integration_id: String,
    source: SourceKind,
) -> Result<CredentialScope, CredentialManagementHttpError> {
    Ok(CredentialScope::integration(
        CredentialOwnerId::new(owner_id).map_err(CredentialManagementHttpError::invalid_input)?,
        source,
        SourceIntegrationId::new(integration_id).map_err(CredentialManagementHttpError::invalid_input)?,
    ))
}

fn source_credential_scope(
    owner_id: String,
    source: SourceKind,
) -> Result<CredentialScope, CredentialManagementHttpError> {
    Ok(CredentialScope::source(
        CredentialOwnerId::new(owner_id).map_err(CredentialManagementHttpError::invalid_input)?,
        source,
    ))
}

fn credential_ref_from_request(
    scope: &CredentialScope,
    kind: CredentialKind,
    version: u64,
) -> Result<CredentialRef, CredentialManagementHttpError> {
    let id = openbao_credential_id(scope, kind).map_err(CredentialManagementHttpError::invalid_input)?;
    let version = CredentialVersion::new(version).map_err(CredentialManagementHttpError::invalid_input)?;
    Ok(CredentialRef::new(id, version, scope, kind))
}

fn command_response(
    stream_position: StreamPosition,
    state: state_v1::CredentialStateSnapshot,
) -> Result<CredentialCommandResponse, CredentialManagementHttpError> {
    match state.state.as_ref() {
        Some(CredentialStateSnapshotCase::Active(active)) => response_for_active("active", stream_position, active),
        Some(CredentialStateSnapshotCase::RotationPending(rotation)) => {
            let active = decode_message_field("rotation_pending.active", &rotation.active)
                .map_err(CredentialManagementHttpError::invalid_state)?;
            response_for_active("rotation_pending", stream_position, active)
        }
        Some(CredentialStateSnapshotCase::Revoked(revoked)) => {
            let credential_ref = decode_revoked_state(revoked).map_err(CredentialManagementHttpError::invalid_state)?;
            Ok(CredentialCommandResponse {
                state: "revoked".to_string(),
                stream_position: stream_position.as_u64(),
                credential_ref: credential_ref_response(&credential_ref, CredentialStatus::Revoked),
            })
        }
        None | Some(CredentialStateSnapshotCase::Missing(_)) => {
            Err(CredentialManagementHttpError::UnexpectedCredentialState("missing"))
        }
        Some(CredentialStateSnapshotCase::PendingWrite(_)) => Err(
            CredentialManagementHttpError::UnexpectedCredentialState("pending_write"),
        ),
        Some(CredentialStateSnapshotCase::WriteFailed(_)) => {
            Err(CredentialManagementHttpError::UnexpectedCredentialState("write_failed"))
        }
    }
}

fn response_for_active(
    state: &'static str,
    stream_position: StreamPosition,
    active: &state_v1::ActiveCredentialState,
) -> Result<CredentialCommandResponse, CredentialManagementHttpError> {
    let (metadata, _previous_versions) =
        decode_active_state("active", active).map_err(CredentialManagementHttpError::invalid_state)?;
    Ok(CredentialCommandResponse {
        state: state.to_string(),
        stream_position: stream_position.as_u64(),
        credential_ref: credential_ref_response(metadata.reference(), metadata.status()),
    })
}

fn credential_ref_response(credential: &CredentialRef, status: CredentialStatus) -> CredentialRefResponse {
    CredentialRefResponse {
        id: credential.id().as_str().to_string(),
        version: credential.version().get(),
        owner_id: credential.owner_id().as_str().to_string(),
        source: credential.source().as_str().to_string(),
        scope_key: credential.scope_key().to_string(),
        kind: credential.kind().as_str().to_string(),
        status: status.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use chrono::Utc;
    use serde_json::Value;
    use tokio::sync::Mutex;
    use tower::ServiceExt;
    use trogon_decider_runtime::{
        AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse,
        ReadStreamRequest, ReadStreamResponse, Snapshot, SnapshotRead, SnapshotWrite, StreamAppend, StreamEvent,
        StreamPosition, StreamRead, StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
    };

    use super::*;
    use crate::credential::processor::recovery_worker::{
        CredentialRecoveryCheckpoint, CredentialRecoveryCheckpointStoreError,
    };
    use crate::credential::processor::runtime_projection::{RuntimeCredentialError, RuntimeIntegrationKey};
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

        async fn append_stream(
            &self,
            request: AppendStreamRequest<'_, str>,
        ) -> Result<AppendStreamResponse, Self::Error> {
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
        assert_eq!(
            body["credential_ref"]["id"],
            "openbao:tenant-1:github/primary:webhook_secret"
        );
        assert_eq!(body["credential_ref"]["version"], 1);
        assert!(!body.to_string().contains("super-secret"));
        assert_eq!(resolved_plaintext(&registry, secrets).await.unwrap(), "super-secret");
        assert_eq!(events.decoded_events().await.len(), 2);
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
        assert_eq!(put_body["credential_ref"]["id"], "openbao:tenant-1:discord:bot_token");
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
            "openbao:tenant-1:slack/primary:signing_secret"
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
            "openbao:tenant-1:gitlab/primary:signing_token"
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
            "openbao:tenant-1:incidentio/primary:signing_secret"
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
            "openbao:tenant-1:linear/primary:webhook_secret"
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
            "openbao:tenant-1:microsoft_graph/primary:client_state"
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
            "openbao:tenant-1:sentry/primary:client_secret"
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
            "openbao:tenant-1:notion/primary:verification_token"
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
            "openbao:tenant-1:telegram/primary:webhook_secret"
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
            "openbao:tenant-1:twitter/primary:consumer_secret"
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
}
