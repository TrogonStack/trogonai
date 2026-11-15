mod constants;
pub(crate) mod idempotency;

use std::error::Error;
use std::time::SystemTime;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use hmac::digest::Key;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::warn;
use trogon_decider_runtime::{CommandError, SnapshotRead, SnapshotWrite, StreamAppend, StreamPosition, StreamRead};
use trogon_std::SecretString;
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1};

use self::constants::{
    ADMIN_TOKEN_HEADER, DISCORD_BOT_TOKEN, GITHUB_WEBHOOK_SECRET, GITLAB_SIGNING_TOKEN, IDEMPOTENCY_KEY_HEADER,
    INCIDENTIO_SIGNING_SECRET, LINEAR_WEBHOOK_SECRET, MICROSOFT_GRAPH_CLIENT_STATE, NOTION_VERIFICATION_TOKEN,
    PLAID_CLIENT_SECRET, SENTRY_CLIENT_SECRET, SLACK_SIGNING_SECRET, TELEGRAM_WEBHOOK_SECRET, TWITTER_CONSUMER_SECRET,
};
#[cfg(test)]
use self::idempotency::CredentialCommandInMemoryIdempotencyLedger;
use self::idempotency::{
    CredentialCommandIdempotencyStore, CredentialCommandIdempotencyStoreError, IdempotencyDecision, IdempotencyKey,
    IdempotencyScope, RequestFingerprint,
};
use crate::credential::commands::domain::{
    CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, CredentialStatus, CredentialVersion, SourceKind,
};
use crate::credential::commands::proto::{
    decode_active_state, decode_cleanup_failed_state, decode_destroy_requested_state, decode_destroyed_state,
    decode_message_field, decode_revoked_state,
};
use crate::credential_lifecycle::handler::{
    CredentialHandlerError, CredentialRuntimeHandler, CredentialRuntimeHandlerError, DestroyStoredCredential,
    PutCredential, RevokeStoredCredential, RotateCredential,
};
use crate::credential_lifecycle::recovery_worker::{
    CredentialRecoveryCheckpoint, CredentialRecoveryCheckpointStore, CredentialRecoveryPolicy,
};
#[cfg(test)]
use crate::credential_runtime_projection::RuntimeCredentialRegistry;
use crate::secret_store::openbao_secret_store::openbao_credential_id;
use crate::secret_store::{
    SecretDestroyReason, SecretStoreDestroy, SecretStoreError, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke,
    SecretStoreRotate,
};
use crate::source_integration_id::SourceIntegrationId;

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

pub(crate) fn router<EventStore, Secrets, Idempotency>(
    handler: CredentialRuntimeHandler<EventStore, Secrets>,
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
        + SecretStoreDestroy<Error = SecretStoreError>
        + SecretStoreMetadata<Error = SecretStoreError>
        + Clone
        + Send
        + Sync
        + 'static,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let state = CredentialManagementState {
        handler,
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
            "/discord/bot-token/destructions",
            post(destroy_discord_bot_token::<EventStore, Secrets, Idempotency>),
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
            "/github/{integration_id}/webhook-secret/destructions",
            post(destroy_github_webhook_secret::<EventStore, Secrets, Idempotency>),
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
            "/gitlab/{integration_id}/signing-token/destructions",
            post(destroy_gitlab_signing_token::<EventStore, Secrets, Idempotency>),
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
            "/incidentio/{integration_id}/signing-secret/destructions",
            post(destroy_incidentio_signing_secret::<EventStore, Secrets, Idempotency>),
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
            "/linear/{integration_id}/webhook-secret/destructions",
            post(destroy_linear_webhook_secret::<EventStore, Secrets, Idempotency>),
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
            "/microsoft-graph/{integration_id}/client-state/destructions",
            post(destroy_microsoft_graph_client_state::<EventStore, Secrets, Idempotency>),
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
            "/notion/{integration_id}/verification-token/destructions",
            post(destroy_notion_verification_token::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/plaid/{integration_id}/client-secret",
            put(put_plaid_client_secret::<EventStore, Secrets, Idempotency>)
                .delete(revoke_plaid_client_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/plaid/{integration_id}/client-secret/rotations",
            post(rotate_plaid_client_secret::<EventStore, Secrets, Idempotency>),
        )
        .route(
            "/plaid/{integration_id}/client-secret/destructions",
            post(destroy_plaid_client_secret::<EventStore, Secrets, Idempotency>),
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
            "/sentry/{integration_id}/client-secret/destructions",
            post(destroy_sentry_client_secret::<EventStore, Secrets, Idempotency>),
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
            "/slack/{integration_id}/signing-secret/destructions",
            post(destroy_slack_signing_secret::<EventStore, Secrets, Idempotency>),
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
            "/telegram/{integration_id}/webhook-secret/destructions",
            post(destroy_telegram_webhook_secret::<EventStore, Secrets, Idempotency>),
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
        .route(
            "/twitter/{integration_id}/consumer-secret/destructions",
            post(destroy_twitter_consumer_secret::<EventStore, Secrets, Idempotency>),
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
        + SecretStoreDestroy<Error = SecretStoreError>
        + SecretStoreMetadata<Error = SecretStoreError>
        + Clone
        + Send
        + Sync
        + 'static,
{
    router(
        CredentialRuntimeHandler::new(event_store, secrets, runtime_credentials),
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
        status: CredentialRecoveryStatus::of(checkpoint, now, policy),
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

async fn put_plaid_client_secret<EventStore, Secrets, Idempotency>(
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
    put_credential_secret(state, path, headers, request, PLAID_CLIENT_SECRET).await
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
    let secret = SecretString::new(secret_value).map_err(CredentialManagementHttpError::invalid_input)?;
    let scope_key = scope.scope_key();
    let fingerprint = request_fingerprint(
        &state.admin_token,
        &[
            CommandNamespace::Create.as_str(),
            scope.owner_id().as_str(),
            scope_key.as_str(),
            credential_id.as_str(),
            target.kind.as_str(),
            secret.as_str(),
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
    let command = PutCredential::new(credential_id, scope, target.kind, secret);

    let outcome = match state.handler.put(command).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(CredentialManagementHttpError::command_failed(error));
        }
    };

    state
        .respond(&idempotency_scope, outcome.stream_position(), outcome.into_state())
        .await
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

async fn rotate_plaid_client_secret<EventStore, Secrets, Idempotency>(
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
    rotate_credential_secret(state, path, headers, request, PLAID_CLIENT_SECRET).await
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
    let secret = SecretString::new(secret_value).map_err(CredentialManagementHttpError::invalid_input)?;
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
            secret.as_str(),
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
    let command = RotateCredential::new(credential_ref, secret);

    let outcome = match state.handler.rotate(command).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(CredentialManagementHttpError::command_failed(error));
        }
    };

    state
        .respond(&idempotency_scope, outcome.stream_position(), outcome.into_state())
        .await
}

async fn destroy_discord_bot_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_source_credential_secret(state, headers, request, DISCORD_BOT_TOKEN).await
}

async fn destroy_github_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, GITHUB_WEBHOOK_SECRET).await
}

async fn destroy_gitlab_signing_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, GITLAB_SIGNING_TOKEN).await
}

async fn destroy_incidentio_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, INCIDENTIO_SIGNING_SECRET).await
}

async fn destroy_slack_signing_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, SLACK_SIGNING_SECRET).await
}

async fn destroy_linear_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, LINEAR_WEBHOOK_SECRET).await
}

async fn destroy_microsoft_graph_client_state<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, MICROSOFT_GRAPH_CLIENT_STATE).await
}

async fn destroy_sentry_client_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, SENTRY_CLIENT_SECRET).await
}

async fn destroy_notion_verification_token<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, NOTION_VERIFICATION_TOKEN).await
}

async fn destroy_plaid_client_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, PLAID_CLIENT_SECRET).await
}

async fn destroy_telegram_webhook_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, TELEGRAM_WEBHOOK_SECRET).await
}

async fn destroy_twitter_consumer_secret<EventStore, Secrets, Idempotency>(
    State(state): State<CredentialManagementState<EventStore, Secrets, Idempotency>>,
    Path(path): Path<ManagedCredentialPath>,
    headers: HeaderMap,
    Json(request): Json<DestroyCredentialSecretRequest>,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    destroy_credential_secret(state, path, headers, request, TWITTER_CONSUMER_SECRET).await
}

async fn destroy_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    path: ManagedCredentialPath,
    headers: HeaderMap,
    request: DestroyCredentialSecretRequest,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = integration_credential_scope(request.owner_id, path.integration_id, target.source)?;
    destroy_scoped_credential_secret(state, scope, headers, request.version, request.reason, target).await
}

async fn destroy_source_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    headers: HeaderMap,
    request: DestroyCredentialSecretRequest,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    let scope = source_credential_scope(request.owner_id, target.source)?;
    destroy_scoped_credential_secret(state, scope, headers, request.version, request.reason, target).await
}

async fn destroy_scoped_credential_secret<EventStore, Secrets, Idempotency>(
    state: CredentialManagementState<EventStore, Secrets, Idempotency>,
    scope: CredentialScope,
    headers: HeaderMap,
    request_version: u64,
    reason: String,
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
    Secrets: SecretStoreDestroy<Error = SecretStoreError>,
    Idempotency: CredentialCommandIdempotencyStore,
{
    authorize(&headers, &state.admin_token)?;
    let idempotency_key = idempotency_key_from_headers(&headers)?;
    let credential_ref = credential_ref_from_request(&scope, target.kind, request_version)?;
    let scope_key = scope.scope_key();
    let reason = SecretDestroyReason::new(reason).map_err(CredentialManagementHttpError::invalid_input)?;
    let version = request_version.to_string();
    let fingerprint = request_fingerprint(
        &state.admin_token,
        &[
            CommandNamespace::Destroy.as_str(),
            scope.owner_id().as_str(),
            scope_key.as_str(),
            credential_ref.id().as_str(),
            version.as_str(),
            target.kind.as_str(),
            reason.as_str(),
        ],
    );
    let idempotency_scope = IdempotencyScope::new(
        scope.owner_id().as_str(),
        CommandNamespace::Destroy.as_str(),
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
    let command = DestroyStoredCredential::new(credential_ref, reason);

    let outcome = match state.handler.destroy(command).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.abandon_idempotency(&idempotency_scope).await;
            return Err(CredentialManagementHttpError::destroy_command_failed(error));
        }
    };

    state
        .respond(&idempotency_scope, outcome.stream_position(), outcome.into_state())
        .await
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

async fn revoke_plaid_client_secret<EventStore, Secrets, Idempotency>(
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
    revoke_credential_secret(state, path, headers, request, PLAID_CLIENT_SECRET).await
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

    state
        .respond(&idempotency_scope, outcome.stream_position(), outcome.into_state())
        .await
}

impl<EventStore, Secrets, Idempotency> CredentialManagementState<EventStore, Secrets, Idempotency>
where
    Idempotency: CredentialCommandIdempotencyStore,
{
    async fn respond(
        &self,
        scope: &IdempotencyScope,
        stream_position: StreamPosition,
        state: state_v1::CredentialStateSnapshot,
    ) -> Result<Json<CredentialCommandResponse>, CredentialManagementHttpError> {
        let response = match command_response(stream_position, state) {
            Ok(response) => response,
            Err(error) => {
                self.abandon_idempotency(scope).await;
                return Err(error);
            }
        };
        self.complete_idempotency(scope, &response).await;
        Ok(Json(response))
    }

    async fn complete_idempotency(&self, scope: &IdempotencyScope, response: &CredentialCommandResponse) {
        let Err(first) = self.idempotency.complete(scope, response.clone()).await else {
            return;
        };
        warn!(error = %first, "credential management idempotency complete failed, retrying");
        if let Err(error) = self.idempotency.complete(scope, response.clone()).await {
            warn!(error = %error, "credential management idempotency complete failed after retry");
        }
    }

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
    Destroy,
}

impl CommandNamespace {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "credential.create",
            Self::Rotate => "credential.rotate",
            Self::Revoke => "credential.revoke",
            Self::Destroy => "credential.destroy",
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

#[derive(Deserialize)]
struct DestroyCredentialSecretRequest {
    owner_id: String,
    version: u64,
    reason: String,
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
    pub(crate) status: CredentialRecoveryStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CredentialRecoveryStatus {
    Idle,
    RetryDelayed,
    Stuck,
}

impl CredentialRecoveryStatus {
    fn of(checkpoint: CredentialRecoveryCheckpoint, now: SystemTime, policy: CredentialRecoveryPolicy) -> Self {
        if checkpoint.stuck_at(now, policy) {
            Self::Stuck
        } else if checkpoint.retry_delayed_at(now) {
            Self::RetryDelayed
        } else {
            Self::Idle
        }
    }
}

#[derive(Debug, Serialize)]
struct CredentialManagementErrorResponse {
    error: &'static str,
}

type BoxedSource = Box<dyn Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
enum CredentialManagementHttpError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("missing idempotency key")]
    MissingIdempotencyKey,
    #[error("idempotency conflict")]
    IdempotencyConflict,
    #[error("idempotency request is already in progress")]
    IdempotencyInProgress,
    #[error("credential management idempotency store failed: {0}")]
    IdempotencyStore(#[source] CredentialCommandIdempotencyStoreError),
    #[error("credential recovery checkpoint failed: {0}")]
    RecoveryCheckpoint(#[source] BoxedSource),
    #[error("credential management input rejected: {0}")]
    InvalidInput(#[source] BoxedSource),
    #[error("credential management command failed: {0}")]
    CommandFailed(#[source] BoxedSource),
    #[error("credential destroy request rejected: {0}")]
    DestroyRejected(#[source] BoxedSource),
    #[error("credential management command returned unexpected state: {0}")]
    UnexpectedCredentialState(&'static str),
    #[error("credential management persisted state is invalid: {0}")]
    InvalidState(#[source] BoxedSource),
}

impl CredentialManagementHttpError {
    fn invalid_input(error: impl Error + Send + Sync + 'static) -> Self {
        Self::InvalidInput(Box::new(error))
    }

    fn invalid_state(error: impl Error + Send + Sync + 'static) -> Self {
        warn!(error = %error, "credential management persisted state is invalid");
        Self::InvalidState(Box::new(error))
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
        Self::CommandFailed(Box::new(error))
    }

    fn destroy_command_failed<SnapshotReadError, ReadError, AppendError>(
        error: CredentialRuntimeHandlerError<SecretStoreError, SnapshotReadError, ReadError, AppendError>,
    ) -> Self
    where
        SnapshotReadError: Error + Send + Sync + 'static,
        ReadError: Error + Send + Sync + 'static,
        AppendError: Error + Send + Sync + 'static,
    {
        match error {
            CredentialRuntimeHandlerError::Command {
                source:
                    CredentialHandlerError::RequestDestroy {
                        source: CommandError::Decide(decide_error),
                    },
            } => {
                warn!(error = %decide_error, "credential destroy request rejected");
                Self::DestroyRejected(Box::new(decide_error))
            }
            error => Self::command_failed(error),
        }
    }

    fn idempotency_store(error: CredentialCommandIdempotencyStoreError) -> Self {
        warn!(error = %error, "credential management idempotency store failed");
        match error {
            CredentialCommandIdempotencyStoreError::Conflict => Self::IdempotencyInProgress,
            source => Self::IdempotencyStore(source),
        }
    }

    fn recovery_checkpoint(error: impl Error + Send + Sync + 'static) -> Self {
        warn!(error = %error, "credential recovery checkpoint status failed");
        Self::RecoveryCheckpoint(Box::new(error))
    }
}

impl IntoResponse for CredentialManagementHttpError {
    fn into_response(self) -> Response {
        let (status, error) = match &self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::MissingIdempotencyKey => (StatusCode::BAD_REQUEST, "missing idempotency key"),
            Self::IdempotencyConflict => (StatusCode::CONFLICT, "idempotency conflict"),
            Self::IdempotencyInProgress => (StatusCode::CONFLICT, "idempotency request is already in progress"),
            Self::IdempotencyStore(_) => {
                warn!(error = %self, "credential management idempotency request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential management idempotency failed",
                )
            }
            Self::RecoveryCheckpoint(_) => {
                warn!(error = %self, "credential recovery checkpoint request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential recovery checkpoint failed",
                )
            }
            Self::InvalidInput(_) => {
                warn!(error = %self, "credential management input rejected");
                (StatusCode::BAD_REQUEST, "invalid credential management request")
            }
            Self::CommandFailed(_) => {
                warn!(error = %self, "credential management request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential management request failed",
                )
            }
            Self::DestroyRejected(_) => {
                warn!(error = %self, "credential destroy request rejected");
                (StatusCode::CONFLICT, "credential destroy request rejected")
            }
            Self::UnexpectedCredentialState(state) => {
                warn!(state, "credential management command returned an unexpected state");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "credential management request failed",
                )
            }
            Self::InvalidState(_) => {
                warn!(error = %self, "credential management persisted state is invalid");
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
    let mut mac = HmacSha256::new(&hmac_key_block(admin_token.as_str().as_bytes()));
    for part in parts {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part.as_bytes());
    }
    RequestFingerprint::new(hex::encode(mac.finalize().into_bytes()))
}

fn hmac_key_block(key: &[u8]) -> Key<HmacSha256> {
    let mut block = Key::<HmacSha256>::default();
    if key.len() > block.len() {
        let digest = Sha256::digest(key);
        block[..digest.len()].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    block
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
        Some(CredentialStateSnapshotCase::DestroyRequested(pending)) => {
            let (credential_ref, _reason) =
                decode_destroy_requested_state(pending).map_err(CredentialManagementHttpError::invalid_state)?;
            Ok(CredentialCommandResponse {
                state: "destroy_requested".to_string(),
                stream_position: stream_position.as_u64(),
                credential_ref: credential_ref_response(&credential_ref, CredentialStatus::Revoked),
            })
        }
        Some(CredentialStateSnapshotCase::Destroyed(destroyed)) => {
            let credential_ref =
                decode_destroyed_state(destroyed).map_err(CredentialManagementHttpError::invalid_state)?;
            Ok(CredentialCommandResponse {
                state: "destroyed".to_string(),
                stream_position: stream_position.as_u64(),
                credential_ref: credential_ref_response(&credential_ref, CredentialStatus::Destroyed),
            })
        }
        Some(CredentialStateSnapshotCase::CleanupFailed(cleanup_failed)) => {
            let (credential_ref, _reason) =
                decode_cleanup_failed_state(cleanup_failed).map_err(CredentialManagementHttpError::invalid_state)?;
            Ok(CredentialCommandResponse {
                state: "cleanup_failed".to_string(),
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
        id: credential.public_id().into_string(),
        version: credential.version().get(),
        owner_id: credential.owner_id().as_str().to_string(),
        source: credential.source().as_str().to_string(),
        scope_key: credential.scope_key().to_string(),
        kind: credential.kind().as_str().to_string(),
        status: status.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests;
