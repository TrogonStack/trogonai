use std::convert::Infallible;
use std::error::Error;
use std::fmt;

use trogon_decider_runtime::{
    CommandError, CommandExecution, EventDecodeOutcome, ReadFrom, ReadStreamRequest, SnapshotRead, SnapshotWrite,
    StreamAppend, StreamPosition, StreamRead, TokioSnapshotTaskScheduler,
};
use trogon_std::SecretString;

use super::domain::{
    CredentialEvent, CredentialEventPayloadError, CredentialId, CredentialIdError, CredentialKind, CredentialOwnerId,
    CredentialRef, CredentialScope, CredentialVersion, SourceKind,
};
use super::{
    ActivateCredentialRotation, ActivateCredentialWrite, CredentialDecideError, CredentialEvolveError,
    CredentialFailureReason, CredentialState, PendingCredentialWrite, RecordCredentialRotationFailure,
    RecordCredentialWriteFailure, RequestCredentialRotation, RequestCredentialWrite, RevokeCredential, evolve,
    initial_state,
};
use crate::credential::processor::runtime_projection::{RuntimeCredentialRegistry, RuntimeProjectionRefreshError};
use crate::secret_store::openbao_secret_store::{OpenBaoCredentialIdParseError, openbao_credential_ref_from_id};
use crate::secret_store::{SecretStoreMetadata, SecretStorePut, SecretStoreRevoke, SecretStoreRotate};

type ExecutionError<SnapshotReadError, ReadError, AppendError> = CommandError<
    CredentialDecideError,
    CredentialEvolveError,
    SnapshotReadError,
    ReadError,
    AppendError,
    Infallible,
    Infallible,
    CredentialEventPayloadError,
>;
type SnapshotReadError<EventStore> = <EventStore as SnapshotRead<CredentialState, str>>::Error;
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
pub(crate) struct CredentialHandler<EventStore, Secrets> {
    event_store: EventStore,
    secrets: Secrets,
}

impl<EventStore, Secrets> CredentialHandler<EventStore, Secrets> {
    pub(crate) fn new(event_store: EventStore, secrets: Secrets) -> Self {
        Self { event_store, secrets }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CredentialHandlerOutcome {
    state: CredentialState,
    stream_position: StreamPosition,
}

impl CredentialHandlerOutcome {
    fn new(state: CredentialState, stream_position: StreamPosition) -> Self {
        Self { state, stream_position }
    }

    pub(crate) fn state(&self) -> &CredentialState {
        &self.state
    }

    pub(crate) fn stream_position(&self) -> StreamPosition {
        self.stream_position
    }

    pub(crate) fn into_state(self) -> CredentialState {
        self.state
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CredentialActivationRecoveryCommand {
    Write(RecoverCredentialWriteActivation),
    Rotation(RecoverCredentialRotationActivation),
}

pub(crate) fn activation_recovery_command(
    stream_id: &str,
    state: &CredentialState,
) -> Result<Option<CredentialActivationRecoveryCommand>, CredentialActivationRecoveryPlanError> {
    match state {
        CredentialState::PendingWrite(pending) => {
            let credential = pending_write_credential_ref(stream_id, pending)?;
            Ok(Some(CredentialActivationRecoveryCommand::Write(
                RecoverCredentialWriteActivation::new(credential),
            )))
        }
        CredentialState::RotationPending(rotation) => Ok(Some(CredentialActivationRecoveryCommand::Rotation(
            RecoverCredentialRotationActivation::new(rotation.active().credential_ref().next_version()),
        ))),
        CredentialState::Missing
        | CredentialState::Active(_)
        | CredentialState::WriteFailed(_)
        | CredentialState::Revoked(_) => Ok(None),
    }
}

fn pending_write_credential_ref(
    stream_id: &str,
    pending: &PendingCredentialWrite,
) -> Result<CredentialRef, CredentialActivationRecoveryPlanError> {
    let credential_id =
        CredentialId::new(stream_id).map_err(CredentialActivationRecoveryPlanError::InvalidCredentialId)?;
    let credential = openbao_credential_ref_from_id(credential_id, CredentialVersion::initial())
        .map_err(CredentialActivationRecoveryPlanError::InvalidOpenBaoCredentialId)?;

    if credential.id() != pending.credential_id()
        || credential.owner_id() != pending.owner_id()
        || credential.source() != pending.source()
        || credential.kind() != pending.kind()
    {
        return Err(CredentialActivationRecoveryPlanError::PendingWriteMismatch {
            credential,
            expected_id: pending.credential_id().clone(),
            expected_owner_id: pending.owner_id().clone(),
            expected_source: pending.source(),
            expected_kind: pending.kind(),
        });
    }

    Ok(credential)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialActivationRecoveryPlanError {
    #[error("credential recovery stream id is invalid: {0}")]
    InvalidCredentialId(#[source] CredentialIdError),
    #[error("credential recovery stream id is not an OpenBao credential id: {0}")]
    InvalidOpenBaoCredentialId(#[source] OpenBaoCredentialIdParseError),
    #[error(
        "credential recovery pending write does not match parsed credential ref: expected {expected_id} {expected_owner_id} {expected_source} {expected_kind}, got {credential}"
    )]
    PendingWriteMismatch {
        credential: CredentialRef,
        expected_id: CredentialId,
        expected_owner_id: CredentialOwnerId,
        expected_source: SourceKind,
        expected_kind: CredentialKind,
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
        + Clone
        + 'static,
    Secrets: SecretStoreRotate + SecretStoreMetadata<Error = <Secrets as SecretStoreRotate>::Error>,
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
        + Clone
        + 'static,
    Secrets: SecretStorePut + SecretStoreMetadata<Error = <Secrets as SecretStorePut>::Error>,
{
    pub(crate) async fn put(
        &self,
        command: PutCredential,
    ) -> HandlerResult<EventStore, <Secrets as SecretStorePut>::Error> {
        let request = RequestCredentialWrite::new(
            command.credential_id.clone(),
            command.scope.owner_id().clone(),
            command.scope.source_kind(),
            command.kind,
        );
        self.ensure_write_requested(&request).await?;

        let credential_ref = match self.secrets.put(command.scope, command.kind, command.value).await {
            Ok(credential_ref) => credential_ref,
            Err(source) => {
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
                let state = load_state(&self.event_store, command.credential_id().as_str()).await?;
                match state {
                    CredentialState::PendingWrite(pending) if pending_matches(&pending, command) => Ok(()),
                    _ => Err(CredentialHandlerError::RequestWrite { source }),
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
        + Clone
        + 'static,
    Secrets: SecretStoreRotate + SecretStoreMetadata<Error = <Secrets as SecretStoreRotate>::Error>,
{
    pub(crate) async fn rotate(
        &self,
        command: RotateCredential,
    ) -> HandlerResult<EventStore, <Secrets as SecretStoreRotate>::Error> {
        let request = RequestCredentialRotation::new(command.credential_ref.clone());
        self.ensure_rotation_requested(&request).await?;

        let rotated_ref = match self.secrets.rotate(&command.credential_ref, command.value).await {
            Ok(rotated_ref) => rotated_ref,
            Err(source) => {
                let reason = failure_reason(&source);
                self.record_rotation_failure(command.credential_ref, reason).await?;
                return Err(CredentialHandlerError::SecretRotate { source });
            }
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
        (),
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
            Ok(_) => Ok(()),
            Err(source) => {
                let state = load_state(&self.event_store, command.credential_ref().id().as_str()).await?;
                match state {
                    CredentialState::RotationPending(rotation)
                        if rotation.active().credential_ref() == command.credential_ref() =>
                    {
                        Ok(())
                    }
                    _ => Err(CredentialHandlerError::RequestRotation { source }),
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
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
        + Clone
        + 'static,
    Secrets: SecretStoreRevoke,
{
    pub(crate) async fn revoke(
        &self,
        command: RevokeStoredCredential,
    ) -> HandlerResult<EventStore, <Secrets as SecretStoreRevoke>::Error> {
        self.secrets
            .revoke(&command.credential_ref)
            .await
            .map_err(|source| CredentialHandlerError::SecretRevoke { source })?;

        let result = CommandExecution::new(&self.event_store, &RevokeCredential::new(command.credential_ref))
            .with_snapshot(&self.event_store)
            .with_task_runtime(TokioSnapshotTaskScheduler)
            .execute()
            .await
            .map_err(|source| CredentialHandlerError::Revoke { source })?;

        Ok(CredentialHandlerOutcome::new(result.state, result.stream_position))
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
}

fn pending_matches(pending: &PendingCredentialWrite, command: &RequestCredentialWrite) -> bool {
    pending.credential_id() == command.credential_id()
        && pending.owner_id() == command.owner_id()
        && pending.source() == command.source()
        && pending.kind() == command.kind()
}

async fn load_state<EventStore, SecretError>(
    event_store: &EventStore,
    stream_id: &str,
) -> Result<
    CredentialState,
    CredentialHandlerError<SecretError, SnapshotReadError<EventStore>, ReadError<EventStore>, AppendError<EventStore>>,
>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<CredentialState, str>
        + SnapshotWrite<CredentialState, str>
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
    let mut state = initial_state();
    for event in stream.events {
        let EventDecodeOutcome::Decoded(event) = event
            .decode::<CredentialEvent>()
            .map_err(|source| CredentialHandlerError::DecodeState { source })?
        else {
            continue;
        };
        state = evolve(state, &event).map_err(|source| CredentialHandlerError::ReplayState { source })?;
    }
    Ok(state)
}

fn failure_reason(source: &impl fmt::Display) -> CredentialFailureReason {
    let value = source.to_string();
    let value = if value.is_empty() {
        "secret store write failed".to_string()
    } else {
        value.chars().take(512).collect()
    };
    CredentialFailureReason::new(value).expect("generated credential failure reason is valid")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use testcontainers_modules::testcontainers::{
        ContainerAsync, GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };
    use trogon_decider_runtime::{
        AppendStreamRequest, AppendStreamResponse, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamResponse,
        Snapshot, SnapshotRead, SnapshotWrite, StreamEvent, StreamPosition, StreamWritePrecondition,
        WriteSnapshotRequest, WriteSnapshotResponse,
    };

    use super::*;
    use crate::credential::domain::{CredentialFingerprint, CredentialMetadata, CredentialStatus, StorageBackend};
    use crate::credential::processor::runtime_projection::{
        RuntimeCredentialError, RuntimeCredentialRegistry, RuntimeIntegrationKey, RuntimeIntegrationProjection,
    };
    use crate::secret_store::openbao_secret_store::OpenBaoSecretStore;
    use crate::secret_store::{MockOpenBaoSecretStore, SecretStoreError, SecretStoreGet};
    use crate::source_integration_id::SourceIntegrationId;

    const OPENBAO_IMAGE: &str = "openbao/openbao";
    const OPENBAO_IMAGE_TAG: &str = "2.5.5";
    const OPENBAO_DEV_ROOT_TOKEN: &str = "dev-only-token";
    const OPENBAO_PORT: u16 = 8200;

    #[derive(Debug, thiserror::Error)]
    #[error("handler test stream store rejected the append")]
    struct HandlerTestStreamStoreError;

    #[derive(Clone, Default)]
    struct HandlerTestStreamStore {
        events: Arc<Mutex<Vec<StreamEvent>>>,
        snapshots: Arc<Mutex<BTreeMap<String, Snapshot<CredentialState>>>>,
        write_preconditions: Arc<Mutex<Vec<StreamWritePrecondition>>>,
        fail_append_at: Arc<Mutex<Option<usize>>>,
    }

    impl HandlerTestStreamStore {
        fn fail_append_at(&self, append_count: usize) {
            *self.fail_append_at.lock().unwrap() = Some(append_count);
        }

        fn events(&self) -> Vec<StreamEvent> {
            self.events.lock().unwrap().clone()
        }

        fn decoded_events(&self) -> Vec<CredentialEvent> {
            self.events()
                .into_iter()
                .map(|event| event.decode::<CredentialEvent>().unwrap().into_decoded().unwrap())
                .collect()
        }

        fn write_preconditions(&self) -> Vec<StreamWritePrecondition> {
            self.write_preconditions.lock().unwrap().clone()
        }
    }

    impl StreamRead<str> for HandlerTestStreamStore {
        type Error = HandlerTestStreamStoreError;

        async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
            let start = match request.from {
                ReadFrom::Beginning => 1,
                ReadFrom::Position(position) => position.as_u64(),
            };
            let events = self.events.lock().unwrap();
            Ok(ReadStreamResponse {
                current_position: current_position(&events, request.stream_id),
                events: events
                    .iter()
                    .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                    .cloned()
                    .collect(),
            })
        }
    }

    impl StreamAppend<str> for HandlerTestStreamStore {
        type Error = HandlerTestStreamStoreError;

        async fn append_stream(
            &self,
            request: AppendStreamRequest<'_, str>,
        ) -> Result<AppendStreamResponse, Self::Error> {
            let mut events = self.events.lock().unwrap();
            let append_count = self.write_preconditions.lock().unwrap().len() + 1;
            self.write_preconditions
                .lock()
                .unwrap()
                .push(request.stream_write_precondition);
            {
                let mut fail_append_at = self.fail_append_at.lock().unwrap();
                if *fail_append_at == Some(append_count) {
                    *fail_append_at = None;
                    return Err(HandlerTestStreamStoreError);
                }
            }

            let current_position = current_position(&events, request.stream_id);
            match request.stream_write_precondition {
                StreamWritePrecondition::Any => {}
                StreamWritePrecondition::StreamExists if current_position.is_some() => {}
                StreamWritePrecondition::NoStream if current_position.is_none() => {}
                StreamWritePrecondition::At(position) if current_position == Some(position) => {}
                _ => return Err(HandlerTestStreamStoreError),
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

    impl SnapshotRead<CredentialState, str> for HandlerTestStreamStore {
        type Error = HandlerTestStreamStoreError;

        async fn read_snapshot(
            &self,
            request: ReadSnapshotRequest<'_, str>,
        ) -> Result<ReadSnapshotResponse<CredentialState>, Self::Error> {
            Ok(ReadSnapshotResponse {
                snapshot: self.snapshots.lock().unwrap().get(request.snapshot_id).cloned(),
            })
        }
    }

    impl SnapshotWrite<CredentialState, str> for HandlerTestStreamStore {
        type Error = HandlerTestStreamStoreError;

        async fn write_snapshot(
            &self,
            request: WriteSnapshotRequest<'_, CredentialState, str>,
        ) -> Result<WriteSnapshotResponse, Self::Error> {
            self.snapshots
                .lock()
                .unwrap()
                .insert(request.snapshot_id.to_string(), request.snapshot);
            Ok(WriteSnapshotResponse)
        }
    }

    #[derive(Clone)]
    struct FailingSecretStore;

    impl SecretStorePut for FailingSecretStore {
        type Error = SecretStoreError;

        async fn put(
            &self,
            scope: CredentialScope,
            kind: CredentialKind,
            _value: SecretString,
        ) -> Result<CredentialRef, Self::Error> {
            let credential = CredentialRef::new(credential_id(), CredentialVersion::initial(), &scope, kind);
            Err(SecretStoreError::BackendUnavailable {
                backend: StorageBackend::OpenBao,
                message: format!("OpenBao refused {}", credential.id()),
            })
        }
    }

    impl SecretStoreMetadata for FailingSecretStore {
        type Error = SecretStoreError;

        async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
            Err(SecretStoreError::Missing {
                credential: credential.clone(),
            })
        }
    }

    #[derive(Clone)]
    struct FailingRotateSecretStore;

    impl SecretStoreRotate for FailingRotateSecretStore {
        type Error = SecretStoreError;

        async fn rotate(&self, credential: &CredentialRef, _value: SecretString) -> Result<CredentialRef, Self::Error> {
            Err(SecretStoreError::BackendUnavailable {
                backend: StorageBackend::OpenBao,
                message: format!("OpenBao refused rotation for {}", credential.id()),
            })
        }
    }

    impl SecretStoreMetadata for FailingRotateSecretStore {
        type Error = SecretStoreError;

        async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
            Err(SecretStoreError::Missing {
                credential: credential.clone(),
            })
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

    fn owner_id() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
    }

    fn integration_id() -> SourceIntegrationId {
        SourceIntegrationId::new("primary").unwrap()
    }

    fn scope() -> CredentialScope {
        CredentialScope::integration(owner_id(), SourceKind::GitHub, integration_id())
    }

    fn credential_id() -> CredentialId {
        CredentialId::new("openbao:tenant-1:github/primary:webhook_secret").unwrap()
    }

    fn source_scoped_credential_id() -> CredentialId {
        CredentialId::new("openbao:tenant-1:github:webhook_secret").unwrap()
    }

    fn runtime_key() -> RuntimeIntegrationKey {
        RuntimeIntegrationKey::new(SourceKind::GitHub, &integration_id())
    }

    fn source_runtime_key() -> RuntimeIntegrationKey {
        RuntimeIntegrationKey::for_source(SourceKind::GitHub)
    }

    fn credential_ref(version: u64) -> CredentialRef {
        CredentialRef::new(
            credential_id(),
            CredentialVersion::new(version).unwrap(),
            &scope(),
            CredentialKind::WebhookSecret,
        )
    }

    fn put_command(value: &str) -> PutCredential {
        PutCredential::new(
            credential_id(),
            scope(),
            CredentialKind::WebhookSecret,
            SecretString::new(value).unwrap(),
        )
    }

    fn source_scoped_put_command(value: &str) -> PutCredential {
        PutCredential::new(
            source_scoped_credential_id(),
            CredentialScope::source(owner_id(), SourceKind::GitHub),
            CredentialKind::WebhookSecret,
            SecretString::new(value).unwrap(),
        )
    }

    fn payload_contains(payload: &[u8], needle: &str) -> bool {
        payload.windows(needle.len()).any(|window| window == needle.as_bytes())
    }

    struct OpenBaoServer {
        _container: ContainerAsync<GenericImage>,
        address: String,
    }

    impl OpenBaoServer {
        async fn start() -> Self {
            let container = GenericImage::new(OPENBAO_IMAGE, OPENBAO_IMAGE_TAG)
                .with_wait_for(WaitFor::message_on_stdout(
                    "Development mode should NOT be used in production",
                ))
                .with_exposed_port(OPENBAO_PORT.tcp())
                .with_cmd(vec![
                    "server",
                    "-dev",
                    "-dev-root-token-id=dev-only-token",
                    "-dev-listen-address=0.0.0.0:8200",
                ])
                .start()
                .await
                .expect("start OpenBao testcontainer");
            let host = container.get_host().await.expect("get OpenBao testcontainer host");
            let port = container
                .get_host_port_ipv4(OPENBAO_PORT)
                .await
                .expect("get OpenBao testcontainer port");
            Self {
                _container: container,
                address: format!("http://{host}:{port}"),
            }
        }

        fn store(&self) -> OpenBaoSecretStore {
            OpenBaoSecretStore::new(&self.address, OPENBAO_DEV_ROOT_TOKEN).unwrap()
        }
    }

    #[test]
    fn recovery_plan_builds_write_activation_command_for_pending_write() {
        let state = evolve(
            initial_state(),
            &CredentialEvent::WriteRequested {
                credential_id: credential_id(),
                owner_id: owner_id(),
                source: SourceKind::GitHub,
                kind: CredentialKind::WebhookSecret,
            },
        )
        .unwrap();

        let command = activation_recovery_command(credential_id().as_str(), &state).unwrap();

        assert_eq!(
            command,
            Some(CredentialActivationRecoveryCommand::Write(
                RecoverCredentialWriteActivation::new(credential_ref(1))
            ))
        );
    }

    #[test]
    fn recovery_plan_builds_rotation_activation_command_for_pending_rotation() {
        let active = CredentialMetadata::new(
            credential_ref(1),
            CredentialStatus::Active,
            StorageBackend::OpenBao,
            CredentialFingerprint::new("fingerprint").unwrap(),
        );
        let state = [
            CredentialEvent::WriteRequested {
                credential_id: credential_id(),
                owner_id: owner_id(),
                source: SourceKind::GitHub,
                kind: CredentialKind::WebhookSecret,
            },
            CredentialEvent::Activated { metadata: active },
            CredentialEvent::RotationRequested {
                credential_ref: credential_ref(1),
            },
        ]
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap();

        let command = activation_recovery_command(credential_id().as_str(), &state).unwrap();

        assert_eq!(
            command,
            Some(CredentialActivationRecoveryCommand::Rotation(
                RecoverCredentialRotationActivation::new(credential_ref(2))
            ))
        );
    }

    #[test]
    fn recovery_plan_skips_completed_state() {
        let active = CredentialMetadata::new(
            credential_ref(1),
            CredentialStatus::Active,
            StorageBackend::OpenBao,
            CredentialFingerprint::new("fingerprint").unwrap(),
        );
        let state = [
            CredentialEvent::WriteRequested {
                credential_id: credential_id(),
                owner_id: owner_id(),
                source: SourceKind::GitHub,
                kind: CredentialKind::WebhookSecret,
            },
            CredentialEvent::Activated { metadata: active },
        ]
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap();

        let command = activation_recovery_command(credential_id().as_str(), &state).unwrap();

        assert_eq!(command, None);
    }

    #[tokio::test]
    async fn put_records_request_writes_secret_and_activates_metadata() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());

        let outcome = handler.put(put_command("super-secret")).await.unwrap();

        assert_eq!(outcome.stream_position(), position(2));
        assert!(matches!(outcome.state(), CredentialState::Active(_)));
        let state = outcome.into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };
        assert_eq!(active.credential_ref().id(), &credential_id());
        assert_eq!(active.metadata().storage_backend(), StorageBackend::OpenBao);
        assert_eq!(
            secrets
                .get(active.credential_ref())
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "super-secret"
        );
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1))
            ]
        );
        assert_eq!(events.decoded_events().len(), 2);
        for event in events.events() {
            assert!(!payload_contains(&event.event.content, "super-secret"));
        }
    }

    #[tokio::test]
    async fn put_records_write_failure_when_secret_store_rejects_the_side_effect() {
        let events = HandlerTestStreamStore::default();
        let handler = CredentialHandler::new(events.clone(), FailingSecretStore);

        let error = handler.put(put_command("ignored-secret")).await.unwrap_err();

        assert!(matches!(error, CredentialHandlerError::SecretWrite { .. }));
        let decoded = events.decoded_events();
        assert_eq!(decoded.len(), 2);
        assert!(matches!(decoded[0], CredentialEvent::WriteRequested { .. }));
        assert!(matches!(decoded[1], CredentialEvent::WriteFailed { .. }));
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1))
            ]
        );
    }

    #[tokio::test]
    async fn put_retry_resumes_pending_write_after_activation_append_failure() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        events.fail_append_at(2);

        let error = handler.put(put_command("first-secret")).await.unwrap_err();

        assert!(matches!(error, CredentialHandlerError::ActivateWrite { .. }));
        assert_eq!(events.decoded_events().len(), 1);

        let outcome = handler.put(put_command("retry-secret")).await.unwrap();

        assert_eq!(outcome.stream_position(), position(2));
        let state = outcome.into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };
        assert_eq!(active.credential_ref().version().get(), 2);
        assert_eq!(
            secrets
                .get(active.credential_ref())
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "retry-secret"
        );
        assert_eq!(events.decoded_events().len(), 2);
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1)),
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1))
            ]
        );
    }

    #[tokio::test]
    async fn recover_write_activation_records_missing_activation_without_plaintext() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        events.fail_append_at(2);

        let error = handler.put(put_command("first-secret")).await.unwrap_err();

        assert!(matches!(error, CredentialHandlerError::ActivateWrite { .. }));
        assert_eq!(events.decoded_events().len(), 1);
        assert_eq!(
            secrets
                .get(&credential_ref(1))
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "first-secret"
        );

        let outcome = handler
            .recover_write_activation(RecoverCredentialWriteActivation::new(credential_ref(1)))
            .await
            .unwrap();

        assert_eq!(outcome.stream_position(), position(2));
        let state = outcome.into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };
        assert_eq!(active.credential_ref(), &credential_ref(1));
        assert_eq!(events.decoded_events().len(), 2);
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1)),
                StreamWritePrecondition::At(position(1))
            ]
        );
        for event in events.events() {
            assert!(!payload_contains(&event.event.content, "first-secret"));
        }
    }

    #[tokio::test]
    async fn rotate_records_request_rotates_secret_and_activates_metadata() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };

        let outcome = handler
            .rotate(RotateCredential::new(
                active.credential_ref().clone(),
                SecretString::new("rotated-secret").unwrap(),
            ))
            .await
            .unwrap();

        assert_eq!(outcome.stream_position(), position(4));
        let state = outcome.into_state();
        let CredentialState::Active(rotated) = state else {
            panic!("expected active credential");
        };
        assert_eq!(rotated.credential_ref().version().get(), 2);
        assert_eq!(rotated.previous_versions(), &[active.credential_ref().clone()]);
        assert_eq!(
            secrets
                .get(rotated.credential_ref())
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "rotated-secret"
        );
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1)),
                StreamWritePrecondition::At(position(2)),
                StreamWritePrecondition::At(position(3))
            ]
        );
        let decoded = events.decoded_events();
        assert!(matches!(decoded[2], CredentialEvent::RotationRequested { .. }));
        assert!(matches!(decoded[3], CredentialEvent::Rotated { .. }));
        for event in events.events() {
            assert!(!payload_contains(&event.event.content, "rotated-secret"));
        }
    }

    #[tokio::test]
    async fn rotate_records_rotation_failure_when_secret_store_rejects_the_side_effect() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let put_handler = CredentialHandler::new(events.clone(), secrets);
        let state = put_handler
            .put(put_command("initial-secret"))
            .await
            .unwrap()
            .into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };
        let rotate_handler = CredentialHandler::new(events.clone(), FailingRotateSecretStore);

        let error = rotate_handler
            .rotate(RotateCredential::new(
                active.credential_ref().clone(),
                SecretString::new("ignored-secret").unwrap(),
            ))
            .await
            .unwrap_err();

        assert!(matches!(error, CredentialHandlerError::SecretRotate { .. }));
        let decoded = events.decoded_events();
        assert_eq!(decoded.len(), 4);
        assert!(matches!(decoded[2], CredentialEvent::RotationRequested { .. }));
        assert!(matches!(decoded[3], CredentialEvent::RotationFailed { .. }));
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1)),
                StreamWritePrecondition::At(position(2)),
                StreamWritePrecondition::At(position(3))
            ]
        );
    }

    #[tokio::test]
    async fn recover_rotation_activation_records_missing_rotation_without_plaintext() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };
        let rotated_ref = active.credential_ref().next_version();
        events.fail_append_at(4);

        let error = handler
            .rotate(RotateCredential::new(
                active.credential_ref().clone(),
                SecretString::new("rotated-secret").unwrap(),
            ))
            .await
            .unwrap_err();

        assert!(matches!(error, CredentialHandlerError::ActivateRotation { .. }));
        assert_eq!(events.decoded_events().len(), 3);
        assert_eq!(
            secrets
                .get(&rotated_ref)
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "rotated-secret"
        );

        let outcome = handler
            .recover_rotation_activation(RecoverCredentialRotationActivation::new(rotated_ref.clone()))
            .await
            .unwrap();

        assert_eq!(outcome.stream_position(), position(4));
        let state = outcome.into_state();
        let CredentialState::Active(rotated) = state else {
            panic!("expected active credential");
        };
        assert_eq!(rotated.credential_ref(), &rotated_ref);
        assert_eq!(rotated.previous_versions(), &[active.credential_ref().clone()]);
        assert_eq!(events.decoded_events().len(), 4);
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1)),
                StreamWritePrecondition::At(position(2)),
                StreamWritePrecondition::At(position(3)),
                StreamWritePrecondition::At(position(3))
            ]
        );
        for event in events.events() {
            assert!(!payload_contains(&event.event.content, "rotated-secret"));
        }
    }

    #[tokio::test]
    async fn revoke_revokes_secret_and_records_state() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };
        let credential_ref = active.credential_ref().clone();

        let outcome = handler
            .revoke(RevokeStoredCredential::new(credential_ref.clone()))
            .await
            .unwrap();

        assert_eq!(outcome.stream_position(), position(3));
        let state = outcome.into_state();
        assert!(matches!(state, CredentialState::Revoked(_)));
        assert!(matches!(
            secrets.get(&credential_ref).await,
            Err(SecretStoreError::Unreadable { .. })
        ));
        let decoded = events.decoded_events();
        assert!(matches!(decoded[2], CredentialEvent::Revoked { .. }));
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1)),
                StreamWritePrecondition::At(position(2))
            ]
        );
    }

    #[tokio::test]
    async fn revoke_retry_records_state_after_append_failure() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        let state = handler.put(put_command("initial-secret")).await.unwrap().into_state();
        let CredentialState::Active(active) = state else {
            panic!("expected active credential");
        };
        let credential_ref = active.credential_ref().clone();
        events.fail_append_at(3);

        let error = handler
            .revoke(RevokeStoredCredential::new(credential_ref.clone()))
            .await
            .unwrap_err();

        assert!(matches!(error, CredentialHandlerError::Revoke { .. }));
        assert_eq!(events.decoded_events().len(), 2);
        assert!(matches!(
            secrets.get(&credential_ref).await,
            Err(SecretStoreError::Unreadable { .. })
        ));

        let outcome = handler
            .revoke(RevokeStoredCredential::new(credential_ref))
            .await
            .unwrap();

        assert_eq!(outcome.stream_position(), position(3));
        let state = outcome.into_state();
        assert!(matches!(state, CredentialState::Revoked(_)));
        assert_eq!(events.decoded_events().len(), 3);
        assert_eq!(
            events.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1)),
                StreamWritePrecondition::At(position(2)),
                StreamWritePrecondition::At(position(2))
            ]
        );
    }

    #[tokio::test]
    async fn runtime_handler_put_updates_runtime_projection() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let resolver = runtime_credentials.resolver(secrets.clone());
        let handler = CredentialRuntimeHandler::new(events.clone(), secrets, runtime_credentials);

        let outcome = handler.put(put_command("super-secret")).await.unwrap();

        assert_eq!(outcome.stream_position(), position(2));
        assert!(matches!(outcome.state(), CredentialState::Active(_)));
        assert_eq!(
            resolver
                .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_str(),
            "super-secret"
        );
    }

    #[tokio::test]
    async fn runtime_handler_put_source_scoped_credential_updates_source_projection() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let resolver = runtime_credentials.resolver(secrets.clone());
        let handler = CredentialRuntimeHandler::new(events.clone(), secrets, runtime_credentials);

        let outcome = handler.put(source_scoped_put_command("source-secret")).await.unwrap();

        assert_eq!(outcome.stream_position(), position(2));
        assert!(matches!(outcome.state(), CredentialState::Active(_)));
        assert_eq!(
            resolver
                .resolve_plaintext(&source_runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_str(),
            "source-secret"
        );
    }

    #[tokio::test]
    async fn runtime_handler_recover_write_activation_updates_runtime_projection() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let resolver = runtime_credentials.resolver(secrets.clone());
        let handler = CredentialRuntimeHandler::new(events.clone(), secrets, runtime_credentials);
        events.fail_append_at(2);

        let error = handler.put(put_command("super-secret")).await.unwrap_err();

        assert!(matches!(
            error,
            CredentialRuntimeHandlerError::Command {
                source: CredentialHandlerError::ActivateWrite { .. }
            }
        ));

        let outcome = handler
            .recover_write_activation(RecoverCredentialWriteActivation::new(credential_ref(1)))
            .await
            .unwrap();

        assert_eq!(outcome.stream_position(), position(2));
        assert_eq!(
            resolver
                .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_str(),
            "super-secret"
        );
    }

    #[tokio::test]
    async fn runtime_handler_revoke_removes_projection_and_invalidates_cached_secret() {
        let events = HandlerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let resolver = runtime_credentials.resolver(secrets.clone());
        let handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
        let outcome = handler.put(put_command("super-secret")).await.unwrap();
        let CredentialState::Active(active) = outcome.into_state() else {
            panic!("expected active credential");
        };
        let credential_ref = active.credential_ref().clone();
        assert_eq!(
            resolver
                .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_str(),
            "super-secret"
        );

        let outcome = handler
            .revoke(RevokeStoredCredential::new(credential_ref.clone()))
            .await
            .unwrap();

        assert_eq!(outcome.stream_position(), position(3));
        assert!(matches!(
            resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
            Err(RuntimeCredentialError::IntegrationNotFound { .. })
        ));

        runtime_credentials
            .projections()
            .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential_ref, 3).unwrap())
            .await;
        assert!(matches!(
            resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
            Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
        ));
    }

    #[tokio::test]
    async fn runtime_handler_with_openbao_testcontainer_applies_state_and_resolves_precise_value() {
        let server = OpenBaoServer::start().await;
        let secrets = server.store();
        let events = HandlerTestStreamStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let resolver = runtime_credentials.resolver(secrets.clone());
        let handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());

        let put = handler.put(put_command("copy-this-value-in-and-out")).await.unwrap();

        assert_eq!(put.stream_position(), position(2));
        let CredentialState::Active(active) = put.into_state() else {
            panic!("expected active credential");
        };
        let initial_ref = active.credential_ref().clone();
        assert_eq!(
            resolver
                .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_str(),
            "copy-this-value-in-and-out"
        );

        let rotated = handler
            .rotate(RotateCredential::new(
                initial_ref.clone(),
                SecretString::new("rotated-copy-this-value-in-and-out").unwrap(),
            ))
            .await
            .unwrap();

        assert_eq!(rotated.stream_position(), position(4));
        let CredentialState::Active(active) = rotated.into_state() else {
            panic!("expected active credential");
        };
        let rotated_ref = active.credential_ref().clone();
        assert_eq!(rotated_ref.version().get(), 2);
        assert_eq!(
            resolver
                .resolve_plaintext(&runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_str(),
            "rotated-copy-this-value-in-and-out"
        );

        let revoked = handler
            .revoke(RevokeStoredCredential::new(rotated_ref.clone()))
            .await
            .unwrap();

        assert_eq!(revoked.stream_position(), position(5));
        assert!(matches!(revoked.state(), CredentialState::Revoked(_)));
        assert!(matches!(
            resolver.resolve(&runtime_key(), CredentialKind::WebhookSecret).await,
            Err(RuntimeCredentialError::IntegrationNotFound { .. })
        ));
        assert!(matches!(
            secrets.get(&rotated_ref).await,
            Err(SecretStoreError::Unreadable { .. })
        ));

        assert_eq!(events.decoded_events().len(), 5);
        for event in events.events() {
            assert!(!payload_contains(&event.event.content, "copy-this-value-in-and-out"));
            assert!(!payload_contains(
                &event.event.content,
                "rotated-copy-this-value-in-and-out"
            ));
        }
    }
}
