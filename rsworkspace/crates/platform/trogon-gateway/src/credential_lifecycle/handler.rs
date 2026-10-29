use std::error::Error;
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::OnceLock;

use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use trogon_decider_runtime::{
    CommandError, CommandExecution, EventDecodeOutcome, ReadFrom, ReadStreamRequest, SnapshotRead, SnapshotWrite,
    StreamAppend, StreamPosition, StreamRead, TokioSnapshotTaskScheduler,
};
use trogon_semconv::{attribute, metric};
use trogon_std::SecretString;
use trogonai_proto::gateway::credentials::{CredentialEventPayloadError, CredentialStateSnapshotCase, state_v1, v1};

use super::constants::CREDENTIAL_LOCK_STRIPES;
use crate::constants::METER_NAME;
use crate::credential::commands::domain::{
    CredentialDestroyReason, CredentialDestroyReasonError, CredentialId, CredentialKind, CredentialRef,
    CredentialScope, CredentialVersion, SourceKind,
};
use crate::credential::commands::proto::{
    CredentialProtoDecodeError, active_credential_ref, decode_destroy_requested_state, decode_destroyed_state,
    decode_message_field, decode_pending_write_state, decode_revoked_state,
};
use crate::credential::{
    ActivateCredentialRotation, ActivateCredentialWrite, CompleteCredentialDestroy, CredentialDecideError,
    CredentialEvolveError, CredentialFailureReason, RecordCredentialDestroyFailure, RecordCredentialRotationFailure,
    RecordCredentialWriteFailure, RequestCredentialDestroy, RequestCredentialRotation, RequestCredentialWrite,
    RevokeCredential, evolve, initial_state,
};
use crate::credential_runtime_projection::{RuntimeCredentialRegistry, RuntimeProjectionRefreshError};
use crate::secret_store::{
    SecretDestroyReason, SecretStoreDestroy, SecretStoreError, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke,
    SecretStoreRotate,
};

type ExecutionError<SnapshotReadError, ReadError, AppendError> = CommandError<
    CredentialDecideError,
    CredentialEvolveError,
    SnapshotReadError,
    ReadError,
    AppendError,
    CredentialEventPayloadError,
    CredentialEventPayloadError,
    CredentialEventPayloadError,
>;
type SnapshotReadError<EventStore> = <EventStore as SnapshotRead<state_v1::CredentialStateSnapshot, str>>::Error;
type ReadError<EventStore> = <EventStore as StreamRead<str>>::Error;
type AppendError<EventStore> = <EventStore as StreamAppend<str>>::Error;
type HandlerResult<EventStore, SecretError> = Result<
    CredentialHandlerOutcome,
    CredentialHandlerError<SecretError, SnapshotReadError<EventStore>, ReadError<EventStore>, AppendError<EventStore>>,
>;
type RuntimeHandlerResult<EventStore, SecretError> = Result<
    CredentialHandlerOutcome,
    CredentialRuntimeHandlerError<
        SecretError,
        SnapshotReadError<EventStore>,
        ReadError<EventStore>,
        AppendError<EventStore>,
    >,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CredentialWriteOperation {
    Put,
    Rotate,
    Revoke,
    Destroy,
}

impl CredentialWriteOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Rotate => "rotate",
            Self::Revoke => "revoke",
            Self::Destroy => "destroy",
        }
    }
}

#[derive(Debug)]
struct CredentialWriteMetrics {
    write_failures: Counter<u64>,
}

impl CredentialWriteMetrics {
    fn new() -> Self {
        let meter = trogon_telemetry::meter(METER_NAME);
        Self {
            write_failures: metric::build_gateway_credential_store_write_failures(&meter),
        }
    }

    /// The store error's message is deliberately not a label. It is derived
    /// from whatever the backend returned, so it is unbounded, and the
    /// responder gets it from the failure event and the logs instead.
    fn record_write_failure(&self, operation: CredentialWriteOperation, source: SourceKind) {
        self.write_failures.add(
            1,
            &[
                KeyValue::new(attribute::OPERATION, operation.as_str()),
                KeyValue::new(attribute::SOURCE, source.as_str()),
            ],
        );
    }
}

static CREDENTIAL_WRITE_METRICS: OnceLock<CredentialWriteMetrics> = OnceLock::new();

fn record_write_failure(operation: CredentialWriteOperation, source: SourceKind) {
    CREDENTIAL_WRITE_METRICS
        .get_or_init(CredentialWriteMetrics::new)
        .record_write_failure(operation, source);
}

#[derive(Clone, Debug)]
pub(crate) struct PutCredential {
    credential_id: CredentialId,
    scope: CredentialScope,
    kind: CredentialKind,
    value: SecretString,
}

#[derive(Clone, Debug)]
pub(crate) struct RotateCredential {
    credential_ref: CredentialRef,
    value: SecretString,
}

impl RotateCredential {
    pub(crate) fn new(credential_ref: CredentialRef, value: SecretString) -> Self {
        Self { credential_ref, value }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoverCredentialWriteActivation {
    credential_ref: CredentialRef,
}

impl RecoverCredentialWriteActivation {
    pub(crate) fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoverCredentialRotationActivation {
    credential_ref: CredentialRef,
}

impl RecoverCredentialRotationActivation {
    pub(crate) fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RevokeStoredCredential {
    credential_ref: CredentialRef,
}

impl RevokeStoredCredential {
    pub(crate) fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DestroyStoredCredential {
    credential_ref: CredentialRef,
    reason: SecretDestroyReason,
}

impl DestroyStoredCredential {
    pub(crate) fn new(credential_ref: CredentialRef, reason: SecretDestroyReason) -> Self {
        Self { credential_ref, reason }
    }
}

impl PutCredential {
    pub(crate) fn new(
        credential_id: CredentialId,
        scope: CredentialScope,
        kind: CredentialKind,
        value: SecretString,
    ) -> Self {
        Self {
            credential_id,
            scope,
            kind,
            value,
        }
    }
}

#[derive(Clone)]
struct CredentialWriteLocks {
    stripes: Arc<[Arc<tokio::sync::Mutex<()>>]>,
}

impl CredentialWriteLocks {
    fn new() -> Self {
        Self {
            stripes: (0..CREDENTIAL_LOCK_STRIPES)
                .map(|_| Arc::new(tokio::sync::Mutex::new(())))
                .collect(),
        }
    }

    async fn acquire(&self, credential_id: &CredentialId) -> tokio::sync::OwnedMutexGuard<()> {
        let mut hasher = DefaultHasher::new();
        credential_id.as_str().hash(&mut hasher);
        let stripe = (hasher.finish() % CREDENTIAL_LOCK_STRIPES as u64) as usize;
        Arc::clone(&self.stripes[stripe]).lock_owned().await
    }
}

#[derive(Clone)]
pub(crate) struct CredentialHandler<EventStore, Secrets> {
    event_store: EventStore,
    secrets: Secrets,
    write_locks: CredentialWriteLocks,
}

impl<EventStore, Secrets> CredentialHandler<EventStore, Secrets> {
    pub(crate) fn new(event_store: EventStore, secrets: Secrets) -> Self {
        Self {
            event_store,
            secrets,
            write_locks: CredentialWriteLocks::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CredentialHandlerOutcome {
    state: state_v1::CredentialStateSnapshot,
    stream_position: StreamPosition,
}

impl CredentialHandlerOutcome {
    fn new(state: state_v1::CredentialStateSnapshot, stream_position: StreamPosition) -> Self {
        Self { state, stream_position }
    }

    pub(crate) fn state(&self) -> &state_v1::CredentialStateSnapshot {
        &self.state
    }

    pub(crate) fn stream_position(&self) -> StreamPosition {
        self.stream_position
    }

    pub(crate) fn into_state(self) -> state_v1::CredentialStateSnapshot {
        self.state
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CredentialActivationRecoveryCommand {
    Write(RecoverCredentialWriteActivation),
    Rotation(RecoverCredentialRotationActivation),
}

pub(crate) fn activation_recovery_command(
    credential_id: &CredentialId,
    state: &state_v1::CredentialStateSnapshot,
    destroyed_version: Option<CredentialVersion>,
) -> Result<Option<CredentialActivationRecoveryCommand>, CredentialActivationRecoveryPlanError> {
    match state.state.as_ref() {
        Some(CredentialStateSnapshotCase::PendingWrite(pending)) => {
            let credential = pending_write_credential_ref(credential_id, pending, destroyed_version)?;
            Ok(Some(CredentialActivationRecoveryCommand::Write(
                RecoverCredentialWriteActivation::new(credential),
            )))
        }
        Some(CredentialStateSnapshotCase::RotationPending(rotation)) => {
            let active = decode_message_field("rotation_pending.active", &rotation.active)
                .map_err(CredentialActivationRecoveryPlanError::InvalidState)?;
            let current_ref =
                active_credential_ref(active).map_err(CredentialActivationRecoveryPlanError::InvalidState)?;
            Ok(Some(CredentialActivationRecoveryCommand::Rotation(
                RecoverCredentialRotationActivation::new(current_ref.next_version()),
            )))
        }
        None
        | Some(
            CredentialStateSnapshotCase::Missing(_)
            | CredentialStateSnapshotCase::Active(_)
            | CredentialStateSnapshotCase::WriteFailed(_)
            | CredentialStateSnapshotCase::Revoked(_)
            | CredentialStateSnapshotCase::DestroyRequested(_)
            | CredentialStateSnapshotCase::Destroyed(_)
            | CredentialStateSnapshotCase::CleanupFailed(_),
        ) => Ok(None),
    }
}

fn pending_write_credential_ref(
    credential_id: &CredentialId,
    pending: &state_v1::PendingCredentialWriteState,
    destroyed_version: Option<CredentialVersion>,
) -> Result<CredentialRef, CredentialActivationRecoveryPlanError> {
    let (pending_id, scope, kind) =
        decode_pending_write_state(pending).map_err(CredentialActivationRecoveryPlanError::InvalidState)?;
    if pending_id != *credential_id {
        return Err(CredentialActivationRecoveryPlanError::PendingWriteMismatch {
            expected: credential_id.clone(),
            actual: pending_id,
        });
    }

    Ok(CredentialRef::new(
        pending_id,
        destroyed_version.map_or_else(CredentialVersion::initial, CredentialVersion::next),
        &scope,
        kind,
    ))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialActivationRecoveryPlanError {
    #[error("credential recovery persisted state is invalid: {0}")]
    InvalidState(#[source] CredentialProtoDecodeError),
    #[error("credential recovery pending write belongs to '{actual}', not stream '{expected}'")]
    PendingWriteMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
}

#[derive(Clone)]
pub(crate) struct CredentialRuntimeHandler<EventStore, Secrets> {
    handler: CredentialHandler<EventStore, Secrets>,
    runtime_credentials: RuntimeCredentialRegistry,
}

impl<EventStore, Secrets> CredentialRuntimeHandler<EventStore, Secrets> {
    pub(crate) fn new(
        event_store: EventStore,
        secrets: Secrets,
        runtime_credentials: RuntimeCredentialRegistry,
    ) -> Self {
        Self {
            handler: CredentialHandler::new(event_store, secrets),
            runtime_credentials,
        }
    }
}

impl<EventStore, Secrets> CredentialRuntimeHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStorePut + SecretStoreMetadata<Error = <Secrets as SecretStorePut>::Error>,
{
    pub(crate) async fn put(
        &self,
        command: PutCredential,
    ) -> RuntimeHandlerResult<EventStore, <Secrets as SecretStorePut>::Error> {
        let outcome = self
            .handler
            .put(command)
            .await
            .map_err(|source| CredentialRuntimeHandlerError::Command { source })?;
        self.apply(&outcome).await?;
        Ok(outcome)
    }
}

impl<EventStore, Secrets> CredentialRuntimeHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
{
    pub(crate) async fn rotate(
        &self,
        command: RotateCredential,
    ) -> RuntimeHandlerResult<EventStore, <Secrets as SecretStoreRotate>::Error> {
        let outcome = self
            .handler
            .rotate(command)
            .await
            .map_err(|source| CredentialRuntimeHandlerError::Command { source })?;
        self.apply(&outcome).await?;
        Ok(outcome)
    }
}

impl<EventStore, Secrets> CredentialRuntimeHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreMetadata,
{
    pub(crate) async fn recover_write_activation(
        &self,
        command: RecoverCredentialWriteActivation,
    ) -> RuntimeHandlerResult<EventStore, <Secrets as SecretStoreMetadata>::Error> {
        let outcome = self
            .handler
            .recover_write_activation(command)
            .await
            .map_err(|source| CredentialRuntimeHandlerError::Command { source })?;
        self.apply(&outcome).await?;
        Ok(outcome)
    }

    pub(crate) async fn recover_rotation_activation(
        &self,
        command: RecoverCredentialRotationActivation,
    ) -> RuntimeHandlerResult<EventStore, <Secrets as SecretStoreMetadata>::Error> {
        let outcome = self
            .handler
            .recover_rotation_activation(command)
            .await
            .map_err(|source| CredentialRuntimeHandlerError::Command { source })?;
        self.apply(&outcome).await?;
        Ok(outcome)
    }
}

impl<EventStore, Secrets> CredentialRuntimeHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreRevoke,
{
    pub(crate) async fn revoke(
        &self,
        command: RevokeStoredCredential,
    ) -> RuntimeHandlerResult<EventStore, <Secrets as SecretStoreRevoke>::Error> {
        let outcome = self
            .handler
            .revoke(command)
            .await
            .map_err(|source| CredentialRuntimeHandlerError::Command { source })?;
        self.apply(&outcome).await?;
        Ok(outcome)
    }
}

impl<EventStore, Secrets> CredentialRuntimeHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreDestroy,
{
    pub(crate) async fn destroy(
        &self,
        command: DestroyStoredCredential,
    ) -> RuntimeHandlerResult<EventStore, <Secrets as SecretStoreDestroy>::Error> {
        let outcome = self
            .handler
            .destroy(command)
            .await
            .map_err(|source| CredentialRuntimeHandlerError::Command { source })?;
        self.apply(&outcome).await?;
        Ok(outcome)
    }
}

impl<EventStore, Secrets> CredentialRuntimeHandler<EventStore, Secrets> {
    async fn apply<SecretError, SnapshotReadError, ReadError, AppendError>(
        &self,
        outcome: &CredentialHandlerOutcome,
    ) -> Result<(), CredentialRuntimeHandlerError<SecretError, SnapshotReadError, ReadError, AppendError>>
    where
        SecretError: Error + Send + Sync + 'static,
        SnapshotReadError: Error + Send + Sync + 'static,
        ReadError: Error + Send + Sync + 'static,
        AppendError: Error + Send + Sync + 'static,
    {
        self.runtime_credentials
            .apply_state(outcome.state(), outcome.stream_position())
            .await
            .map_err(|source| CredentialRuntimeHandlerError::Projection { source })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialRuntimeHandlerError<SecretError, SnapshotReadError, ReadError, AppendError>
where
    SecretError: Error + Send + Sync + 'static,
    SnapshotReadError: Error + Send + Sync + 'static,
    ReadError: Error + Send + Sync + 'static,
    AppendError: Error + Send + Sync + 'static,
{
    #[error("credential command failed: {source}")]
    Command {
        #[source]
        source: CredentialHandlerError<SecretError, SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential runtime projection failed: {source}")]
    Projection {
        #[source]
        source: RuntimeProjectionRefreshError,
    },
}

impl<EventStore, Secrets> CredentialHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreMetadata,
{
    pub(crate) async fn recover_write_activation(
        &self,
        command: RecoverCredentialWriteActivation,
    ) -> HandlerResult<EventStore, <Secrets as SecretStoreMetadata>::Error> {
        let metadata = self
            .secrets
            .metadata(&command.credential_ref)
            .await
            .map_err(|source| CredentialHandlerError::SecretMetadata { source })?;
        let result = CommandExecution::new(&self.event_store, &ActivateCredentialWrite::new(metadata))
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
            .map_err(|source| CredentialHandlerError::RecoverWriteActivation { source })?;

        Ok(CredentialHandlerOutcome::new(result.state, result.stream_position))
    }

    pub(crate) async fn recover_rotation_activation(
        &self,
        command: RecoverCredentialRotationActivation,
    ) -> HandlerResult<EventStore, <Secrets as SecretStoreMetadata>::Error> {
        let metadata = self
            .secrets
            .metadata(&command.credential_ref)
            .await
            .map_err(|source| CredentialHandlerError::SecretMetadata { source })?;
        let result = CommandExecution::new(&self.event_store, &ActivateCredentialRotation::new(metadata))
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
            .map_err(|source| CredentialHandlerError::RecoverRotationActivation { source })?;

        Ok(CredentialHandlerOutcome::new(result.state, result.stream_position))
    }
}

impl<EventStore, Secrets> CredentialHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStorePut + SecretStoreMetadata<Error = <Secrets as SecretStorePut>::Error>,
{
    pub(crate) async fn put(
        &self,
        command: PutCredential,
    ) -> HandlerResult<EventStore, <Secrets as SecretStorePut>::Error> {
        let _write_guard = self.write_locks.acquire(&command.credential_id).await;
        let request = RequestCredentialWrite::new(command.credential_id.clone(), command.scope.clone(), command.kind);
        self.ensure_write_requested(&request).await?;

        let source_kind = command.scope.source_kind();
        let credential_ref = match self.secrets.put(command.scope, command.kind, command.value).await {
            Ok(credential_ref) => credential_ref,
            Err(source) => {
                record_write_failure(CredentialWriteOperation::Put, source_kind);
                let reason = failure_reason(&source);
                self.record_write_failure(command.credential_id, reason).await?;
                return Err(CredentialHandlerError::SecretWrite { source });
            }
        };
        let metadata = self
            .secrets
            .metadata(&credential_ref)
            .await
            .map_err(|source| CredentialHandlerError::SecretMetadata { source })?;
        let result = CommandExecution::new(&self.event_store, &ActivateCredentialWrite::new(metadata))
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
            .map_err(|source| CredentialHandlerError::ActivateWrite { source })?;

        Ok(CredentialHandlerOutcome::new(result.state, result.stream_position))
    }

    async fn ensure_write_requested(
        &self,
        command: &RequestCredentialWrite,
    ) -> Result<
        (),
        CredentialHandlerError<
            <Secrets as SecretStorePut>::Error,
            SnapshotReadError<EventStore>,
            ReadError<EventStore>,
            AppendError<EventStore>,
        >,
    > {
        match CommandExecution::new(&self.event_store, command)
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
        {
            Ok(_) => Ok(()),
            Err(source) => {
                let (state, _stream_position) = load_state(&self.event_store, command.credential_id().as_str()).await?;
                let matches = match state.state.as_ref() {
                    Some(CredentialStateSnapshotCase::PendingWrite(pending)) => pending_matches(pending, command)
                        .map_err(|source| CredentialHandlerError::InvalidState { source })?,
                    _ => false,
                };
                if matches {
                    Ok(())
                } else {
                    Err(CredentialHandlerError::RequestWrite { source })
                }
            }
        }
    }

    async fn record_write_failure(
        &self,
        credential_id: CredentialId,
        reason: CredentialFailureReason,
    ) -> Result<
        (),
        CredentialHandlerError<
            <Secrets as SecretStorePut>::Error,
            SnapshotReadError<EventStore>,
            ReadError<EventStore>,
            AppendError<EventStore>,
        >,
    > {
        CommandExecution::new(
            &self.event_store,
            &RecordCredentialWriteFailure::new(credential_id, reason),
        )
        .with_snapshot(&self.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .map(|_| ())
        .map_err(|source| CredentialHandlerError::RecordWriteFailure { source })
    }
}

impl<EventStore, Secrets> CredentialHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreRotate<Error = SecretStoreError> + SecretStoreMetadata<Error = SecretStoreError>,
{
    pub(crate) async fn rotate(
        &self,
        command: RotateCredential,
    ) -> HandlerResult<EventStore, <Secrets as SecretStoreRotate>::Error> {
        let _write_guard = self.write_locks.acquire(command.credential_ref.id()).await;
        let request = RequestCredentialRotation::new(command.credential_ref.clone());
        let resumed = self.ensure_rotation_requested(&request).await?;

        let already_written = if resumed {
            let next_ref = command.credential_ref.next_version();
            match self.secrets.metadata(&next_ref).await {
                Ok(_) => Some(next_ref),
                Err(SecretStoreError::Missing { .. }) => None,
                Err(source) => return Err(CredentialHandlerError::SecretMetadata { source }),
            }
        } else {
            None
        };
        let rotated_ref = match already_written {
            Some(rotated_ref) => rotated_ref,
            None => match self.secrets.rotate(&command.credential_ref, command.value).await {
                Ok(rotated_ref) => rotated_ref,
                Err(source) => {
                    record_write_failure(CredentialWriteOperation::Rotate, command.credential_ref.source());
                    let reason = failure_reason(&source);
                    self.record_rotation_failure(command.credential_ref, reason).await?;
                    return Err(CredentialHandlerError::SecretRotate { source });
                }
            },
        };
        let metadata = self
            .secrets
            .metadata(&rotated_ref)
            .await
            .map_err(|source| CredentialHandlerError::SecretMetadata { source })?;
        let result = CommandExecution::new(&self.event_store, &ActivateCredentialRotation::new(metadata))
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
            .map_err(|source| CredentialHandlerError::ActivateRotation { source })?;

        Ok(CredentialHandlerOutcome::new(result.state, result.stream_position))
    }

    async fn ensure_rotation_requested(
        &self,
        command: &RequestCredentialRotation,
    ) -> Result<
        bool,
        CredentialHandlerError<
            <Secrets as SecretStoreRotate>::Error,
            SnapshotReadError<EventStore>,
            ReadError<EventStore>,
            AppendError<EventStore>,
        >,
    > {
        match CommandExecution::new(&self.event_store, command)
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
        {
            Ok(_) => Ok(false),
            Err(source) => {
                let (state, _stream_position) =
                    load_state(&self.event_store, command.credential_ref().id().as_str()).await?;
                let matches = match state.state.as_ref() {
                    Some(CredentialStateSnapshotCase::RotationPending(rotation)) => {
                        let active = decode_message_field("rotation_pending.active", &rotation.active)
                            .map_err(|source| CredentialHandlerError::InvalidState { source })?;
                        let current_ref = active_credential_ref(active)
                            .map_err(|source| CredentialHandlerError::InvalidState { source })?;
                        current_ref == *command.credential_ref()
                    }
                    _ => false,
                };
                if matches {
                    Ok(true)
                } else {
                    Err(CredentialHandlerError::RequestRotation { source })
                }
            }
        }
    }

    async fn record_rotation_failure(
        &self,
        credential_ref: CredentialRef,
        reason: CredentialFailureReason,
    ) -> Result<
        (),
        CredentialHandlerError<
            <Secrets as SecretStoreRotate>::Error,
            SnapshotReadError<EventStore>,
            ReadError<EventStore>,
            AppendError<EventStore>,
        >,
    > {
        CommandExecution::new(
            &self.event_store,
            &RecordCredentialRotationFailure::new(credential_ref, reason),
        )
        .with_snapshot(&self.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .map(|_| ())
        .map_err(|source| CredentialHandlerError::RecordRotationFailure { source })
    }
}

impl<EventStore, Secrets> CredentialHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreRevoke,
{
    pub(crate) async fn revoke(
        &self,
        command: RevokeStoredCredential,
    ) -> HandlerResult<EventStore, <Secrets as SecretStoreRevoke>::Error> {
        let revoke = RevokeCredential::new(command.credential_ref.clone());
        let (state, stream_position) = load_state(&self.event_store, command.credential_ref.id().as_str()).await?;
        let already_revoked = is_revoked_as(&state, &command.credential_ref)?;
        if already_revoked {
            let stream_position = stream_position.ok_or(CredentialHandlerError::MissingRevokedStreamPosition)?;
            return Ok(CredentialHandlerOutcome::new(state, stream_position));
        }
        revoke
            .ensure_revocable(&state)
            .map_err(|source| CredentialHandlerError::Revoke {
                source: CommandError::Decide(source),
            })?;

        self.secrets.revoke(&command.credential_ref).await.map_err(|source| {
            record_write_failure(CredentialWriteOperation::Revoke, command.credential_ref.source());
            CredentialHandlerError::SecretRevoke { source }
        })?;

        let mut expected = stream_position;
        loop {
            match CommandExecution::new(&self.event_store, &revoke)
                .with_snapshot(&self.event_store)
                .with_task_runtime(TokioSnapshotTaskScheduler)
                .with_expected_revision(expected)
                .execute()
                .await
            {
                Ok(result) => return Ok(CredentialHandlerOutcome::new(result.state, result.stream_position)),
                Err(source) => {
                    let (state, fresh_position) =
                        load_state(&self.event_store, command.credential_ref.id().as_str()).await?;
                    if is_revoked_as(&state, &command.credential_ref)? {
                        let fresh_position =
                            fresh_position.ok_or(CredentialHandlerError::MissingRevokedStreamPosition)?;
                        return Ok(CredentialHandlerOutcome::new(state, fresh_position));
                    }
                    if fresh_position == expected || revoke.ensure_revocable(&state).is_err() {
                        return Err(CredentialHandlerError::Revoke { source });
                    }
                    expected = fresh_position;
                }
            }
        }
    }
}

enum DestroyRequestOutcome {
    Proceed,
    AlreadyDestroyed(CredentialHandlerOutcome),
}

impl<EventStore, Secrets> CredentialHandler<EventStore, Secrets>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreDestroy,
{
    pub(crate) async fn destroy(
        &self,
        command: DestroyStoredCredential,
    ) -> HandlerResult<EventStore, <Secrets as SecretStoreDestroy>::Error> {
        let reason = CredentialDestroyReason::new(command.reason.as_str())
            .map_err(|source| CredentialHandlerError::InvalidDestroyReason { source })?;
        let request = RequestCredentialDestroy::new(command.credential_ref.clone(), reason);
        match self.ensure_destroy_requested(&request).await? {
            DestroyRequestOutcome::AlreadyDestroyed(outcome) => return Ok(outcome),
            DestroyRequestOutcome::Proceed => {}
        }

        match self.secrets.destroy(&command.credential_ref, &command.reason).await {
            Ok(()) => {}
            Err(source) => {
                record_write_failure(CredentialWriteOperation::Destroy, command.credential_ref.source());
                let reason = failure_reason(&source);
                self.record_destroy_failure(command.credential_ref, reason).await?;
                return Err(CredentialHandlerError::SecretDestroy { source });
            }
        }

        let result = CommandExecution::new(
            &self.event_store,
            &CompleteCredentialDestroy::new(command.credential_ref),
        )
        .with_snapshot(&self.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .map_err(|source| CredentialHandlerError::CompleteDestroy { source })?;

        Ok(CredentialHandlerOutcome::new(result.state, result.stream_position))
    }

    async fn ensure_destroy_requested(
        &self,
        command: &RequestCredentialDestroy,
    ) -> Result<
        DestroyRequestOutcome,
        CredentialHandlerError<
            <Secrets as SecretStoreDestroy>::Error,
            SnapshotReadError<EventStore>,
            ReadError<EventStore>,
            AppendError<EventStore>,
        >,
    > {
        match CommandExecution::new(&self.event_store, command)
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
        {
            Ok(_) => Ok(DestroyRequestOutcome::Proceed),
            Err(source) => {
                let (state, stream_position) =
                    load_state(&self.event_store, command.credential_ref().id().as_str()).await?;

                let pending_matches = match state.state.as_ref() {
                    Some(CredentialStateSnapshotCase::DestroyRequested(pending)) => {
                        let (current_ref, _) = decode_destroy_requested_state(pending)
                            .map_err(|source| CredentialHandlerError::InvalidState { source })?;
                        current_ref == *command.credential_ref()
                    }
                    _ => false,
                };
                if pending_matches {
                    return Ok(DestroyRequestOutcome::Proceed);
                }

                let already_destroyed = match state.state.as_ref() {
                    Some(CredentialStateSnapshotCase::Destroyed(destroyed)) => {
                        let current_ref = decode_destroyed_state(destroyed)
                            .map_err(|source| CredentialHandlerError::InvalidState { source })?;
                        current_ref == *command.credential_ref()
                    }
                    _ => false,
                };
                if already_destroyed {
                    let stream_position =
                        stream_position.ok_or(CredentialHandlerError::MissingDestroyedStreamPosition)?;
                    return Ok(DestroyRequestOutcome::AlreadyDestroyed(CredentialHandlerOutcome::new(
                        state,
                        stream_position,
                    )));
                }

                Err(CredentialHandlerError::RequestDestroy { source })
            }
        }
    }

    async fn record_destroy_failure(
        &self,
        credential_ref: CredentialRef,
        reason: CredentialFailureReason,
    ) -> Result<
        (),
        CredentialHandlerError<
            <Secrets as SecretStoreDestroy>::Error,
            SnapshotReadError<EventStore>,
            ReadError<EventStore>,
            AppendError<EventStore>,
        >,
    > {
        CommandExecution::new(
            &self.event_store,
            &RecordCredentialDestroyFailure::new(credential_ref, reason),
        )
        .with_snapshot(&self.event_store)
        .with_task_runtime(TokioSnapshotTaskScheduler)
        .execute()
        .await
        .map(|_| ())
        .map_err(|source| CredentialHandlerError::RecordDestroyFailure { source })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialHandlerError<SecretError, SnapshotReadError, ReadError, AppendError>
where
    SecretError: Error + Send + Sync + 'static,
    SnapshotReadError: Error + Send + Sync + 'static,
    ReadError: Error + Send + Sync + 'static,
    AppendError: Error + Send + Sync + 'static,
{
    #[error("credential write request failed: {source}")]
    RequestWrite {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential state read failed: {source}")]
    ReadState {
        #[source]
        source: ReadError,
    },
    #[error("credential state decode failed: {source}")]
    DecodeState {
        #[source]
        source: CredentialEventPayloadError,
    },
    #[error("persisted credential state is invalid: {source}")]
    InvalidState {
        #[source]
        source: CredentialProtoDecodeError,
    },
    #[error("credential state replay failed: {source}")]
    ReplayState {
        #[source]
        source: CredentialEvolveError,
    },
    #[error("secret store write failed: {source}")]
    SecretWrite {
        #[source]
        source: SecretError,
    },
    #[error("secret store metadata read failed: {source}")]
    SecretMetadata {
        #[source]
        source: SecretError,
    },
    #[error("credential write failure recording failed: {source}")]
    RecordWriteFailure {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential rotation request failed: {source}")]
    RequestRotation {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("secret store rotate failed: {source}")]
    SecretRotate {
        #[source]
        source: SecretError,
    },
    #[error("credential rotation failure recording failed: {source}")]
    RecordRotationFailure {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential write activation failed: {source}")]
    ActivateWrite {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential rotation activation failed: {source}")]
    ActivateRotation {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential write activation recovery failed: {source}")]
    RecoverWriteActivation {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential rotation activation recovery failed: {source}")]
    RecoverRotationActivation {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("secret store revoke failed: {source}")]
    SecretRevoke {
        #[source]
        source: SecretError,
    },
    #[error("credential revoke failed: {source}")]
    Revoke {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential destroy reason is invalid: {source}")]
    InvalidDestroyReason {
        #[source]
        source: CredentialDestroyReasonError,
    },
    #[error("credential destroy request failed: {source}")]
    RequestDestroy {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("secret store destroy failed: {source}")]
    SecretDestroy {
        #[source]
        source: SecretError,
    },
    #[error("credential destroy failure recording failed: {source}")]
    RecordDestroyFailure {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential destroy completion failed: {source}")]
    CompleteDestroy {
        #[source]
        source: ExecutionError<SnapshotReadError, ReadError, AppendError>,
    },
    #[error("credential was revoked but its persisted state carried no stream position")]
    MissingRevokedStreamPosition,
    #[error("credential was destroyed but its persisted state carried no stream position")]
    MissingDestroyedStreamPosition,
}

fn is_revoked_as<SecretError, SnapshotReadError, ReadError, AppendError>(
    state: &state_v1::CredentialStateSnapshot,
    credential_ref: &CredentialRef,
) -> Result<bool, CredentialHandlerError<SecretError, SnapshotReadError, ReadError, AppendError>>
where
    SecretError: Error + Send + Sync + 'static,
    SnapshotReadError: Error + Send + Sync + 'static,
    ReadError: Error + Send + Sync + 'static,
    AppendError: Error + Send + Sync + 'static,
{
    match state.state.as_ref() {
        Some(CredentialStateSnapshotCase::Revoked(revoked)) => decode_revoked_state(revoked)
            .map(|current_ref| current_ref == *credential_ref)
            .map_err(|source| CredentialHandlerError::InvalidState { source }),
        _ => Ok(false),
    }
}

fn pending_matches(
    pending: &state_v1::PendingCredentialWriteState,
    command: &RequestCredentialWrite,
) -> Result<bool, CredentialProtoDecodeError> {
    let (credential_id, scope, kind) = decode_pending_write_state(pending)?;
    Ok(credential_id == *command.credential_id() && scope == *command.scope() && kind == command.kind())
}

async fn load_state<EventStore, SecretError>(
    event_store: &EventStore,
    stream_id: &str,
) -> Result<
    (state_v1::CredentialStateSnapshot, Option<StreamPosition>),
    CredentialHandlerError<SecretError, SnapshotReadError<EventStore>, ReadError<EventStore>, AppendError<EventStore>>,
>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    SecretError: Error + Send + Sync + 'static,
{
    let stream = event_store
        .read_stream(ReadStreamRequest {
            stream_id,
            from: ReadFrom::Beginning,
        })
        .await
        .map_err(|source| CredentialHandlerError::ReadState { source })?;
    let current_position = stream.current_position;
    let mut state = initial_state();
    for event in stream.events {
        let EventDecodeOutcome::Decoded(event) = event
            .decode::<v1::CredentialEvent>()
            .map_err(|source| CredentialHandlerError::DecodeState { source })?
        else {
            continue;
        };
        state = evolve(state, &event).map_err(|source| CredentialHandlerError::ReplayState { source })?;
    }
    Ok((state, current_position))
}

fn failure_reason(source: &impl fmt::Display) -> CredentialFailureReason {
    let value: String = source.to_string().chars().take(512).collect();
    CredentialFailureReason::new(value).unwrap_or_else(|_| CredentialFailureReason::secret_store_write_failed())
}

#[cfg(test)]
mod tests;
