#![allow(dead_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{self, context, kv};
use buffa::Message as _;
use bytes::Bytes;
use tokio::sync::Mutex;
use tracing::{error, info};
use trogon_decider_nats::StreamStoreError;
use trogon_decider_runtime::{
    EventDecodeOutcome, InvalidStreamPosition, ReadFrom, ReadStreamRequest, StreamEvent, StreamPosition, StreamRead,
};
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamGetRawMessage, JetStreamGetStreamInfo,
    JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry,
    is_create_key_value_already_exists,
};
use trogon_std::SecretString;
use trogonai_proto::gateway::credentials::checkpoints_v1 as proto;

use crate::credential::commands::domain::{CredentialId, CredentialKind, CredentialOwnerId, CredentialRef, SourceKind};
use crate::credential::{
    CredentialEvent, CredentialEventPayloadError, CredentialEvolveError, CredentialState, evolve, initial_state,
};
use crate::secret_store::{SecretMaterial, SecretStoreError, SecretStoreGet};
use crate::source_integration_id::{SourceIntegrationId, SourceIntegrationIdError};

const RUNTIME_PROJECTION_CHECKPOINT_KEY: &str = "v1.runtime-projection";
pub(crate) const CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINT_BUCKET: &str =
    "GATEWAY_CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINTS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeIntegrationStatus {
    Active,
    Disabled,
    Archived,
    Deleted,
    Pending,
    Failed,
}

impl RuntimeIntegrationStatus {
    fn is_resolvable(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RuntimeIntegrationKey {
    source: SourceKind,
    scope: RuntimeIntegrationScope,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RuntimeIntegrationScope {
    Source,
    Integration(String),
}

impl RuntimeIntegrationKey {
    pub fn new(source: SourceKind, integration_id: &SourceIntegrationId) -> Self {
        Self {
            source,
            scope: RuntimeIntegrationScope::Integration(integration_id.as_str().to_string()),
        }
    }

    pub fn for_source(source: SourceKind) -> Self {
        Self {
            source,
            scope: RuntimeIntegrationScope::Source,
        }
    }

    pub fn source(&self) -> SourceKind {
        self.source
    }

    pub fn integration_id(&self) -> Option<&str> {
        match &self.scope {
            RuntimeIntegrationScope::Source => None,
            RuntimeIntegrationScope::Integration(integration_id) => Some(integration_id),
        }
    }

    pub fn from_credential_ref(credential: &CredentialRef) -> Result<Self, RuntimeProjectionBuildError> {
        let scope_key = credential.scope_key();
        if scope_key == credential.source().as_str() {
            return Ok(Self::for_source(credential.source()));
        }

        let source_prefix = format!("{}/", credential.source().as_str());
        let Some(integration_id) = scope_key.strip_prefix(&source_prefix) else {
            return Err(RuntimeProjectionBuildError::ScopeSourceMismatch {
                credential: credential.clone(),
                scope_key: scope_key.to_string(),
                source_kind: credential.source(),
            });
        };
        let integration_id = SourceIntegrationId::new(integration_id).map_err(|source| {
            RuntimeProjectionBuildError::InvalidIntegrationId {
                credential: credential.clone(),
                source,
            }
        })?;

        Ok(Self::new(credential.source(), &integration_id))
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeIntegrationProjection {
    key: RuntimeIntegrationKey,
    owner_id: CredentialOwnerId,
    status: RuntimeIntegrationStatus,
    version: u64,
    credentials: BTreeMap<CredentialKind, CredentialRef>,
}

impl RuntimeIntegrationProjection {
    pub fn new(
        owner_id: CredentialOwnerId,
        source: SourceKind,
        integration_id: SourceIntegrationId,
        status: RuntimeIntegrationStatus,
        version: u64,
    ) -> Self {
        Self {
            key: RuntimeIntegrationKey::new(source, &integration_id),
            owner_id,
            status,
            version,
            credentials: BTreeMap::new(),
        }
    }

    pub fn with_credential(mut self, kind: CredentialKind, credential: CredentialRef) -> Self {
        self.credentials.insert(kind, credential);
        self
    }

    fn insert_credential(&mut self, credential: CredentialRef) {
        self.credentials.insert(credential.kind(), credential);
    }

    fn remove_credential(&mut self, kind: CredentialKind) -> Option<CredentialRef> {
        self.credentials.remove(&kind)
    }

    fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }

    fn advance_version(&mut self, version: u64) {
        self.version = self.version.max(version);
    }

    pub fn active_from_credential_ref(
        credential: CredentialRef,
        version: u64,
    ) -> Result<Self, RuntimeProjectionBuildError> {
        let key = RuntimeIntegrationKey::from_credential_ref(&credential)?;
        Ok(Self {
            key,
            owner_id: credential.owner_id().clone(),
            status: RuntimeIntegrationStatus::Active,
            version,
            credentials: BTreeMap::from([(credential.kind(), credential)]),
        })
    }

    pub fn from_credential_state(
        state: &CredentialState,
        version: u64,
    ) -> Result<Option<Self>, RuntimeProjectionBuildError> {
        match state {
            CredentialState::Active(active) => active_runtime_projection(active.credential_ref().clone(), version),
            CredentialState::RotationPending(rotation) => {
                active_runtime_projection(rotation.active().credential_ref().clone(), version)
            }
            CredentialState::Missing
            | CredentialState::PendingWrite(_)
            | CredentialState::WriteFailed(_)
            | CredentialState::Revoked(_) => Ok(None),
        }
    }

    pub fn key(&self) -> &RuntimeIntegrationKey {
        &self.key
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn status(&self) -> RuntimeIntegrationStatus {
        self.status
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    fn credential(&self, kind: CredentialKind) -> Option<&CredentialRef> {
        self.credentials.get(&kind)
    }
}

fn active_runtime_projection(
    credential: CredentialRef,
    version: u64,
) -> Result<Option<RuntimeIntegrationProjection>, RuntimeProjectionBuildError> {
    let key = RuntimeIntegrationKey::from_credential_ref(&credential)?;
    Ok(Some(RuntimeIntegrationProjection {
        key,
        owner_id: credential.owner_id().clone(),
        status: RuntimeIntegrationStatus::Active,
        version,
        credentials: BTreeMap::from([(credential.kind(), credential)]),
    }))
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeProjectionBuildError {
    #[error("credential scope key '{scope_key}' does not match source '{source_kind}' for {credential}")]
    ScopeSourceMismatch {
        credential: CredentialRef,
        scope_key: String,
        source_kind: SourceKind,
    },
    #[error("credential ref has invalid integration id: {credential}")]
    InvalidIntegrationId {
        credential: CredentialRef,
        #[source]
        source: SourceIntegrationIdError,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeProjectionRefreshReport {
    scanned_events: usize,
    decoded_events: usize,
    skipped_events: usize,
    changed_credentials: usize,
    applied_credentials: usize,
    projected_integrations: usize,
    checkpoint_loaded_sequence: u64,
    checkpoint_advanced_to: Option<u64>,
}

impl RuntimeProjectionRefreshReport {
    pub fn scanned_events(self) -> usize {
        self.scanned_events
    }

    pub fn decoded_events(self) -> usize {
        self.decoded_events
    }

    pub fn skipped_events(self) -> usize {
        self.skipped_events
    }

    pub fn changed_credentials(self) -> usize {
        self.changed_credentials
    }

    pub fn applied_credentials(self) -> usize {
        self.applied_credentials
    }

    pub fn projected_integrations(self) -> usize {
        self.projected_integrations
    }

    pub fn checkpoint_loaded_sequence(self) -> u64 {
        self.checkpoint_loaded_sequence
    }

    pub fn checkpoint_advanced_to(self) -> Option<u64> {
        self.checkpoint_advanced_to
    }

    fn has_projection_activity(self) -> bool {
        self.scanned_events > 0
            || self.decoded_events > 0
            || self.skipped_events > 0
            || self.changed_credentials > 0
            || self.applied_credentials > 0
            || self.projected_integrations > 0
            || self.checkpoint_advanced_to.is_some()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeProjectionRefreshError {
    #[error("credential event decode failed: {source}")]
    DecodeEvent {
        #[source]
        source: CredentialEventPayloadError,
    },
    #[error("credential stream replay failed: {source}")]
    ReplayStream {
        #[source]
        source: CredentialEvolveError,
    },
    #[error("credential stream read failed for {credential_id}: {source}")]
    ReadCredential {
        credential_id: CredentialId,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("runtime projection stream position is invalid: {source}")]
    InvalidStreamPosition {
        #[source]
        source: InvalidStreamPosition,
    },
    #[error("runtime projection could not be built: {source}")]
    BuildProjection {
        #[source]
        source: RuntimeProjectionBuildError,
    },
    #[error("runtime projection owner mismatch for {key}: expected {expected}, got {actual}")]
    OwnerMismatch {
        key: RuntimeIntegrationKey,
        expected: CredentialOwnerId,
        actual: CredentialOwnerId,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeProjectionStreamRefreshError {
    #[error("credential event stream read failed: {source}")]
    ReadStream {
        #[source]
        source: StreamStoreError,
    },
    #[error("runtime projection refresh failed: {source}")]
    Refresh {
        #[source]
        source: RuntimeProjectionRefreshError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeProjectionCheckpointedRefreshError {
    #[error("runtime projection checkpoint failed: {source}")]
    Checkpoint {
        #[source]
        source: RuntimeProjectionCheckpointStoreError,
    },
    #[error("runtime projection stream refresh failed: {source}")]
    Refresh {
        #[source]
        source: RuntimeProjectionStreamRefreshError,
    },
}

pub async fn run_checkpointed_refresh_worker<EventStream, EventStore, Checkpoints>(
    registry: RuntimeCredentialRegistry,
    event_stream: EventStream,
    event_store: EventStore,
    checkpoints: Checkpoints,
    interval: Duration,
) where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    Checkpoints: RuntimeProjectionCheckpointStore,
{
    let mut interval = tokio::time::interval(interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = trogon_std::signal::shutdown_signal() => {
                info!("credential runtime projection refresh worker stopped");
                return;
            }
            _ = interval.tick() => {
                match registry
                    .refresh_from_credential_stream_checkpointed(&event_stream, &event_store, &checkpoints)
                    .await
                {
                    Ok(report) if report.has_projection_activity() => {
                        info!(
                            scanned_events = report.scanned_events(),
                            decoded_events = report.decoded_events(),
                            skipped_events = report.skipped_events(),
                            changed_credentials = report.changed_credentials(),
                            applied_credentials = report.applied_credentials(),
                            projected_integrations = report.projected_integrations(),
                            checkpoint_loaded_sequence = report.checkpoint_loaded_sequence(),
                            checkpoint_advanced_to = ?report.checkpoint_advanced_to(),
                            "credential runtime projection refresh pass completed"
                        );
                    }
                    Ok(_) => {}
                    Err(source) => {
                        error!(error = %source, "credential runtime projection refresh pass failed");
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeProjectionCheckpoint {
    last_scanned_sequence: u64,
}

impl RuntimeProjectionCheckpoint {
    pub fn new(last_scanned_sequence: u64) -> Self {
        Self { last_scanned_sequence }
    }

    pub fn last_scanned_sequence(self) -> u64 {
        self.last_scanned_sequence
    }

    fn next_sequence(self) -> u64 {
        self.last_scanned_sequence.saturating_add(1).max(1)
    }
}

pub trait RuntimeProjectionCheckpointStore: Clone + Send + Sync + 'static {
    fn load(
        &self,
    ) -> impl std::future::Future<Output = Result<RuntimeProjectionCheckpoint, RuntimeProjectionCheckpointStoreError>> + Send;

    fn save(
        &self,
        checkpoint: RuntimeProjectionCheckpoint,
    ) -> impl std::future::Future<Output = Result<(), RuntimeProjectionCheckpointStoreError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeProjectionCheckpointStoreError {
    #[error("runtime projection checkpoint codec failed: {source}")]
    Codec {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("runtime projection checkpoint backend failed: {source}")]
    Backend {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("runtime projection checkpoint changed concurrently")]
    Conflict,
}

impl RuntimeProjectionCheckpointStoreError {
    fn backend(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend {
            source: Box::new(source),
        }
    }

    fn codec(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Codec {
            source: Box::new(source),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeProjectionKvCheckpointStore<S> {
    kv: S,
}

impl<S> RuntimeProjectionKvCheckpointStore<S>
where
    S: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    pub fn new(kv: S) -> Self {
        Self { kv }
    }
}

impl<S> RuntimeProjectionCheckpointStore for RuntimeProjectionKvCheckpointStore<S>
where
    S: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    async fn load(&self) -> Result<RuntimeProjectionCheckpoint, RuntimeProjectionCheckpointStoreError> {
        let Some(entry) = self
            .kv
            .entry(RUNTIME_PROJECTION_CHECKPOINT_KEY.to_string())
            .await
            .map_err(RuntimeProjectionCheckpointStoreError::backend)?
        else {
            return Ok(RuntimeProjectionCheckpoint::default());
        };

        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(RuntimeProjectionCheckpoint::default());
        }

        decode_projection_checkpoint(&entry.value)
    }

    async fn save(&self, checkpoint: RuntimeProjectionCheckpoint) -> Result<(), RuntimeProjectionCheckpointStoreError> {
        let encoded = Bytes::from(encode_projection_checkpoint(checkpoint));
        for _ in 0..3 {
            let Some(entry) = self
                .kv
                .entry(RUNTIME_PROJECTION_CHECKPOINT_KEY.to_string())
                .await
                .map_err(RuntimeProjectionCheckpointStoreError::backend)?
            else {
                match self.kv.create(RUNTIME_PROJECTION_CHECKPOINT_KEY, encoded.clone()).await {
                    Ok(_) => return Ok(()),
                    Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(RuntimeProjectionCheckpointStoreError::backend(source)),
                }
            };

            if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
                match self.kv.create(RUNTIME_PROJECTION_CHECKPOINT_KEY, encoded.clone()).await {
                    Ok(_) => return Ok(()),
                    Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(RuntimeProjectionCheckpointStoreError::backend(source)),
                }
            }

            match self
                .kv
                .update(RUNTIME_PROJECTION_CHECKPOINT_KEY, encoded.clone(), entry.revision)
                .await
            {
                Ok(_) => return Ok(()),
                Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(source) => return Err(RuntimeProjectionCheckpointStoreError::backend(source)),
            }
        }

        Err(RuntimeProjectionCheckpointStoreError::Conflict)
    }
}

pub async fn open_runtime_projection_checkpoint_store(
    context: jetstream::Context,
) -> Result<RuntimeProjectionKvCheckpointStore<kv::Store>, RuntimeProjectionCheckpointOpenError> {
    let store = provision_runtime_projection_checkpoint_bucket::<_, kv::Store>(&context).await?;
    Ok(RuntimeProjectionKvCheckpointStore::new(store))
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeProjectionCheckpointOpenError {
    #[error("failed to create runtime projection checkpoint bucket: {0}")]
    Create(#[source] Box<context::CreateKeyValueError>),
    #[error("failed to open existing runtime projection checkpoint bucket: {0}")]
    OpenExisting(#[source] Box<context::KeyValueError>),
    #[error("failed to inspect runtime projection checkpoint bucket: {0}")]
    Inspect(#[source] kv::StatusError),
    #[error("{source}")]
    Incompatible {
        #[source]
        source: IncompatibleRuntimeProjectionCheckpointBucket,
    },
}

#[derive(Debug, thiserror::Error)]
#[error(
    "runtime projection checkpoint bucket is incompatible: expected history {expected_history}, got {actual_history}; expected max age {expected_max_age:?}, got {actual_max_age:?}"
)]
pub struct IncompatibleRuntimeProjectionCheckpointBucket {
    expected_history: i64,
    actual_history: i64,
    expected_max_age: Duration,
    actual_max_age: Duration,
}

pub async fn provision_runtime_projection_checkpoint_bucket<C, S>(
    client: &C,
) -> Result<S, RuntimeProjectionCheckpointOpenError>
where
    C: JetStreamCreateKeyValue<Store = S> + JetStreamGetKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let store = match client
        .create_key_value(runtime_projection_checkpoint_bucket_config())
        .await
    {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => client
            .get_key_value(CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINT_BUCKET)
            .await
            .map_err(|source| RuntimeProjectionCheckpointOpenError::OpenExisting(Box::new(source)))?,
        Err(source) => return Err(RuntimeProjectionCheckpointOpenError::Create(Box::new(source))),
    };

    validate_runtime_projection_checkpoint_bucket(&store).await?;
    Ok(store)
}

pub fn runtime_projection_checkpoint_bucket_config() -> kv::Config {
    kv::Config {
        bucket: CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINT_BUCKET.to_string(),
        history: 1,
        max_age: Duration::ZERO,
        ..Default::default()
    }
}

async fn validate_runtime_projection_checkpoint_bucket<S>(store: &S) -> Result<(), RuntimeProjectionCheckpointOpenError>
where
    S: JetStreamKeyValueStatus,
{
    let status = store
        .status()
        .await
        .map_err(RuntimeProjectionCheckpointOpenError::Inspect)?;
    let history = status.history();
    let max_age = status.max_age();
    if history != 1 || max_age != Duration::ZERO {
        return Err(RuntimeProjectionCheckpointOpenError::Incompatible {
            source: IncompatibleRuntimeProjectionCheckpointBucket {
                expected_history: 1,
                actual_history: history,
                expected_max_age: Duration::ZERO,
                actual_max_age: max_age,
            },
        });
    }
    Ok(())
}

fn encode_projection_checkpoint(checkpoint: RuntimeProjectionCheckpoint) -> Vec<u8> {
    proto::RuntimeProjectionCheckpoint {
        last_scanned_sequence: Some(checkpoint.last_scanned_sequence()),
    }
    .encode_to_vec()
}

fn decode_projection_checkpoint(
    value: &[u8],
) -> Result<RuntimeProjectionCheckpoint, RuntimeProjectionCheckpointStoreError> {
    let checkpoint = proto::RuntimeProjectionCheckpoint::decode_from_slice(value)
        .map_err(RuntimeProjectionCheckpointStoreError::codec)?;
    Ok(RuntimeProjectionCheckpoint::new(
        checkpoint.last_scanned_sequence.unwrap_or_default(),
    ))
}

#[derive(Clone, Default)]
pub struct InMemoryRuntimeProjectionRepository {
    projections: Arc<Mutex<BTreeMap<RuntimeIntegrationKey, RuntimeIntegrationProjection>>>,
}

impl InMemoryRuntimeProjectionRepository {
    pub async fn upsert(&self, projection: RuntimeIntegrationProjection) {
        self.projections
            .lock()
            .await
            .insert(projection.key().clone(), projection);
    }

    pub async fn replace_all(&self, projections: BTreeMap<RuntimeIntegrationKey, RuntimeIntegrationProjection>) {
        *self.projections.lock().await = projections;
    }

    pub async fn len(&self) -> usize {
        self.projections.lock().await.len()
    }

    pub async fn remove(&self, key: &RuntimeIntegrationKey) {
        self.projections.lock().await.remove(key);
    }

    pub async fn merge(&self, projection: RuntimeIntegrationProjection) -> Result<(), RuntimeProjectionRefreshError> {
        let mut projections = self.projections.lock().await;
        merge_projection(&mut projections, projection)
    }

    pub async fn remove_credential(&self, key: &RuntimeIntegrationKey, kind: CredentialKind) -> Option<CredentialRef> {
        let mut projections = self.projections.lock().await;
        let projection = projections.get_mut(key)?;
        let removed = projection.remove_credential(kind);
        if projection.is_empty() {
            projections.remove(key);
        }
        removed
    }

    async fn get(&self, key: &RuntimeIntegrationKey) -> Option<RuntimeIntegrationProjection> {
        self.projections.lock().await.get(key).cloned()
    }
}

#[derive(Clone, Default)]
pub struct RuntimeCredentialRegistry {
    projections: InMemoryRuntimeProjectionRepository,
    cache: RuntimeCredentialCache,
}

impl RuntimeCredentialRegistry {
    pub fn projections(&self) -> &InMemoryRuntimeProjectionRepository {
        &self.projections
    }

    pub fn cache(&self) -> &RuntimeCredentialCache {
        &self.cache
    }

    pub fn resolver<S>(&self, store: S) -> RuntimeCredentialResolver<S> {
        RuntimeCredentialResolver::with_cache(self.projections.clone(), self.cache.clone(), store)
    }

    pub async fn refresh_from_credential_stream<S>(
        &self,
        stream: &S,
        from_sequence: u64,
    ) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionStreamRefreshError>
    where
        S: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    {
        refresh_runtime_projections_from_credential_stream(&self.projections, &self.cache, stream, from_sequence).await
    }

    pub async fn refresh_from_credential_stream_checkpointed<EventStream, EventStore, Checkpoints>(
        &self,
        event_stream: &EventStream,
        event_store: &EventStore,
        checkpoints: &Checkpoints,
    ) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionCheckpointedRefreshError>
    where
        EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
        EventStore: StreamRead<str>,
        <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
        Checkpoints: RuntimeProjectionCheckpointStore,
    {
        refresh_runtime_projections_from_credential_stream_checkpointed(
            &self.projections,
            &self.cache,
            event_stream,
            event_store,
            checkpoints,
        )
        .await
    }

    pub async fn refresh_from_credential_stream_incremental<EventStream, EventStore>(
        &self,
        event_stream: &EventStream,
        event_store: &EventStore,
        from_sequence: u64,
    ) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionStreamRefreshError>
    where
        EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
        EventStore: StreamRead<str>,
        <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    {
        refresh_runtime_projections_from_credential_stream_incremental(
            &self.projections,
            &self.cache,
            event_stream,
            event_store,
            from_sequence,
        )
        .await
    }

    pub async fn refresh_from_credential_events(
        &self,
        events: impl IntoIterator<Item = StreamEvent>,
    ) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionRefreshError> {
        refresh_runtime_projections_from_credential_events(&self.projections, &self.cache, events).await
    }

    pub async fn apply_state(
        &self,
        state: &CredentialState,
        stream_position: StreamPosition,
    ) -> Result<(), RuntimeProjectionRefreshError> {
        if let Some(projection) = RuntimeIntegrationProjection::from_credential_state(state, stream_position.as_u64())
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?
        {
            self.projections.merge(projection).await?;
            self.cache.clear().await;
            return Ok(());
        }

        if let CredentialState::Revoked(revoked) = state {
            self.remove_credential_ref(revoked.credential_ref()).await?;
        }
        Ok(())
    }

    async fn remove_credential_ref(&self, credential: &CredentialRef) -> Result<(), RuntimeProjectionRefreshError> {
        self.cache.invalidate(credential).await;
        let key = RuntimeIntegrationKey::from_credential_ref(credential)
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?;
        self.projections.remove_credential(&key, credential.kind()).await;
        Ok(())
    }
}

pub async fn refresh_runtime_projections_from_credential_stream<S>(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    stream: &S,
    from_sequence: u64,
) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionStreamRefreshError>
where
    S: JetStreamGetStreamInfo + JetStreamGetRawMessage,
{
    let events = trogon_decider_nats::read_stream(stream, from_sequence)
        .await
        .map_err(|source| RuntimeProjectionStreamRefreshError::ReadStream { source })?;
    refresh_runtime_projections_from_credential_events(projections, cache, events)
        .await
        .map_err(|source| RuntimeProjectionStreamRefreshError::Refresh { source })
}

pub async fn refresh_runtime_projections_from_credential_stream_checkpointed<EventStream, EventStore, Checkpoints>(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    event_stream: &EventStream,
    event_store: &EventStore,
    checkpoints: &Checkpoints,
) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionCheckpointedRefreshError>
where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    Checkpoints: RuntimeProjectionCheckpointStore,
{
    let checkpoint = checkpoints
        .load()
        .await
        .map_err(|source| RuntimeProjectionCheckpointedRefreshError::Checkpoint { source })?;
    let mut report = refresh_runtime_projections_from_credential_stream_incremental(
        projections,
        cache,
        event_stream,
        event_store,
        checkpoint.next_sequence(),
    )
    .await
    .map_err(|source| RuntimeProjectionCheckpointedRefreshError::Refresh { source })?;
    report.checkpoint_loaded_sequence = checkpoint.last_scanned_sequence();

    if let Some(last_scanned_sequence) = report.checkpoint_advanced_to() {
        checkpoints
            .save(RuntimeProjectionCheckpoint::new(last_scanned_sequence))
            .await
            .map_err(|source| RuntimeProjectionCheckpointedRefreshError::Checkpoint { source })?;
    }

    Ok(report)
}

pub async fn refresh_runtime_projections_from_credential_stream_incremental<EventStream, EventStore>(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    event_stream: &EventStream,
    event_store: &EventStore,
    from_sequence: u64,
) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionStreamRefreshError>
where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
{
    let events = trogon_decider_nats::read_stream(event_stream, from_sequence)
        .await
        .map_err(|source| RuntimeProjectionStreamRefreshError::ReadStream { source })?;
    refresh_runtime_projections_from_changed_credential_events(projections, cache, event_store, events)
        .await
        .map_err(|source| RuntimeProjectionStreamRefreshError::Refresh { source })
}

pub async fn refresh_runtime_projections_from_credential_events(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    events: impl IntoIterator<Item = StreamEvent>,
) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionRefreshError> {
    let mut report = RuntimeProjectionRefreshReport::default();
    let mut streams: BTreeMap<String, Vec<(u64, CredentialEvent)>> = BTreeMap::new();

    for event in events {
        report.scanned_events += 1;
        let stream_id = event.stream_id().to_string();
        let stream_position = event.stream_position.as_u64();
        match event
            .decode::<CredentialEvent>()
            .map_err(|source| RuntimeProjectionRefreshError::DecodeEvent { source })?
        {
            EventDecodeOutcome::Decoded(event) => {
                report.decoded_events += 1;
                streams.entry(stream_id).or_default().push((stream_position, event));
            }
            EventDecodeOutcome::Skipped => {
                report.skipped_events += 1;
            }
        }
    }

    let mut next_projections = BTreeMap::new();
    for (_stream_id, mut stream_events) in streams {
        stream_events.sort_by_key(|(position, _)| *position);
        let Some(version) = stream_events.last().map(|(position, _)| *position) else {
            continue;
        };
        let state = stream_events
            .into_iter()
            .map(|(_, event)| event)
            .try_fold(initial_state(), |state, event| evolve(state, &event))
            .map_err(|source| RuntimeProjectionRefreshError::ReplayStream { source })?;

        let Some(projection) = RuntimeIntegrationProjection::from_credential_state(&state, version)
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?
        else {
            continue;
        };
        merge_projection(&mut next_projections, projection)?;
    }

    report.projected_integrations = next_projections.len();
    cache.clear().await;
    projections.replace_all(next_projections).await;
    Ok(report)
}

async fn refresh_runtime_projections_from_changed_credential_events<EventStore>(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    event_store: &EventStore,
    events: impl IntoIterator<Item = StreamEvent>,
) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionRefreshError>
where
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
{
    let mut report = RuntimeProjectionRefreshReport::default();
    let mut changed_credentials = BTreeMap::<CredentialId, u64>::new();

    for event in events {
        report.scanned_events += 1;
        let stream_position = event.stream_position.as_u64();
        report.checkpoint_advanced_to = Some(report.checkpoint_advanced_to.unwrap_or(0).max(stream_position));
        match event
            .decode::<CredentialEvent>()
            .map_err(|source| RuntimeProjectionRefreshError::DecodeEvent { source })?
        {
            EventDecodeOutcome::Decoded(event) => {
                report.decoded_events += 1;
                let credential_id = event_credential_id(&event).clone();
                changed_credentials
                    .entry(credential_id)
                    .and_modify(|position| *position = (*position).max(stream_position))
                    .or_insert(stream_position);
            }
            EventDecodeOutcome::Skipped => {
                report.skipped_events += 1;
            }
        }
    }

    report.changed_credentials = changed_credentials.len();

    for (credential_id, version) in changed_credentials {
        let state = load_credential_state(event_store, &credential_id).await?;
        apply_state_to_projection(projections, cache, &state, position(version)?).await?;
        report.applied_credentials += 1;
    }
    report.projected_integrations = projections.len().await;

    Ok(report)
}

async fn load_credential_state<EventStore>(
    event_store: &EventStore,
    credential_id: &CredentialId,
) -> Result<CredentialState, RuntimeProjectionRefreshError>
where
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
{
    let stream = event_store
        .read_stream(ReadStreamRequest {
            stream_id: credential_id.as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .map_err(|source| RuntimeProjectionRefreshError::ReadCredential {
            credential_id: credential_id.clone(),
            source: Box::new(source),
        })?;
    let mut state = initial_state();
    for event in stream.events {
        let EventDecodeOutcome::Decoded(event) = event
            .decode::<CredentialEvent>()
            .map_err(|source| RuntimeProjectionRefreshError::DecodeEvent { source })?
        else {
            continue;
        };
        state = evolve(state, &event).map_err(|source| RuntimeProjectionRefreshError::ReplayStream { source })?;
    }
    Ok(state)
}

async fn apply_state_to_projection(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    state: &CredentialState,
    stream_position: StreamPosition,
) -> Result<(), RuntimeProjectionRefreshError> {
    if let Some(projection) = RuntimeIntegrationProjection::from_credential_state(state, stream_position.as_u64())
        .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?
    {
        projections.merge(projection).await?;
        cache.clear().await;
        return Ok(());
    }

    if let CredentialState::Revoked(revoked) = state {
        cache.invalidate(revoked.credential_ref()).await;
        let key = RuntimeIntegrationKey::from_credential_ref(revoked.credential_ref())
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?;
        projections
            .remove_credential(&key, revoked.credential_ref().kind())
            .await;
    }
    Ok(())
}

fn event_credential_id(event: &CredentialEvent) -> &CredentialId {
    match event {
        CredentialEvent::WriteRequested { credential_id, .. } | CredentialEvent::WriteFailed { credential_id, .. } => {
            credential_id
        }
        CredentialEvent::Activated { metadata } => metadata.reference().id(),
        CredentialEvent::RotationRequested { credential_ref }
        | CredentialEvent::RotationFailed { credential_ref, .. }
        | CredentialEvent::Revoked { credential_ref } => credential_ref.id(),
        CredentialEvent::Rotated {
            previous_credential_ref,
            ..
        } => previous_credential_ref.id(),
    }
}

fn position(value: u64) -> Result<StreamPosition, RuntimeProjectionRefreshError> {
    StreamPosition::try_new(value).map_err(|source| RuntimeProjectionRefreshError::InvalidStreamPosition { source })
}

fn merge_projection(
    projections: &mut BTreeMap<RuntimeIntegrationKey, RuntimeIntegrationProjection>,
    projection: RuntimeIntegrationProjection,
) -> Result<(), RuntimeProjectionRefreshError> {
    let key = projection.key().clone();
    match projections.get_mut(&key) {
        Some(existing) => {
            if existing.owner_id() != projection.owner_id() {
                return Err(RuntimeProjectionRefreshError::OwnerMismatch {
                    key,
                    expected: existing.owner_id().clone(),
                    actual: projection.owner_id().clone(),
                });
            }
            let version = projection.version();
            for credential in projection.credentials.into_values() {
                existing.insert_credential(credential);
            }
            existing.advance_version(version);
        }
        None => {
            projections.insert(key, projection);
        }
    }
    Ok(())
}

#[derive(Clone, Default)]
pub struct RuntimeCredentialCache {
    entries: Arc<Mutex<BTreeMap<CredentialRef, SecretMaterial>>>,
}

impl RuntimeCredentialCache {
    async fn get(&self, credential: &CredentialRef) -> Option<SecretMaterial> {
        self.entries.lock().await.get(credential).cloned()
    }

    async fn put(&self, credential: CredentialRef, material: SecretMaterial) {
        self.entries.lock().await.insert(credential, material);
    }

    pub async fn invalidate(&self, credential: &CredentialRef) {
        self.entries.lock().await.remove(credential);
    }

    pub async fn clear(&self) {
        self.entries.lock().await.clear();
    }
}

#[derive(Clone)]
pub struct RuntimeCredentialResolver<S> {
    projections: InMemoryRuntimeProjectionRepository,
    cache: RuntimeCredentialCache,
    store: S,
}

impl<S> RuntimeCredentialResolver<S> {
    pub fn new(projections: InMemoryRuntimeProjectionRepository, store: S) -> Self {
        Self::with_cache(projections, RuntimeCredentialCache::default(), store)
    }

    pub fn with_cache(
        projections: InMemoryRuntimeProjectionRepository,
        cache: RuntimeCredentialCache,
        store: S,
    ) -> Self {
        Self {
            projections,
            cache,
            store,
        }
    }

    pub fn cache(&self) -> &RuntimeCredentialCache {
        &self.cache
    }
}

impl<S> RuntimeCredentialResolver<S>
where
    S: SecretStoreGet<Error = SecretStoreError>,
{
    pub async fn resolve(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
    ) -> Result<SecretMaterial, RuntimeCredentialError> {
        let projection = self
            .projections
            .get(key)
            .await
            .ok_or_else(|| RuntimeCredentialError::IntegrationNotFound { key: key.clone() })?;

        if !projection.status().is_resolvable() {
            return Err(RuntimeCredentialError::IntegrationNotResolvable {
                key: key.clone(),
                status: projection.status(),
            });
        }

        let credential = projection
            .credential(kind)
            .ok_or_else(|| RuntimeCredentialError::CredentialMissing { key: key.clone(), kind })?;

        if let Some(material) = self.cache.get(credential).await {
            return Ok(material);
        }

        let material = self.store.get(credential).await?;
        self.cache.put(credential.clone(), material.clone()).await;
        Ok(material)
    }

    pub async fn resolve_plaintext(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
    ) -> Result<SecretString, RuntimeCredentialError> {
        match self.resolve(key, kind).await? {
            SecretMaterial::Plaintext(value) => Ok(value),
            SecretMaterial::Verifier(_) => Err(RuntimeCredentialError::VerifierOnly { key: key.clone(), kind }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeCredentialError {
    #[error("runtime integration not found: {key}")]
    IntegrationNotFound { key: RuntimeIntegrationKey },
    #[error("runtime integration is not resolvable: {key} is {status:?}")]
    IntegrationNotResolvable {
        key: RuntimeIntegrationKey,
        status: RuntimeIntegrationStatus,
    },
    #[error("runtime credential missing: {key} {kind}")]
    CredentialMissing {
        key: RuntimeIntegrationKey,
        kind: CredentialKind,
    },
    #[error("runtime credential is verifier-only: {key} {kind}")]
    VerifierOnly {
        key: RuntimeIntegrationKey,
        kind: CredentialKind,
    },
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
}

impl RuntimeCredentialError {
    pub fn is_secret_store_error(&self) -> bool {
        matches!(self, Self::SecretStore(_))
    }
}

impl std::fmt::Display for RuntimeIntegrationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.scope {
            RuntimeIntegrationScope::Source => write!(f, "{}", self.source),
            RuntimeIntegrationScope::Integration(integration_id) => write!(f, "{}/{}", self.source, integration_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::credential::commands::domain::{CredentialScope, CredentialVersion};
    use crate::secret_store::{
        MockOpenBaoSecretStore, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
    };
    use chrono::Utc;
    use trogon_decider_nats::{StreamSubject, append_stream};
    use trogon_decider_runtime::{
        Event, EventEncode, EventId, EventType, Headers, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamEvent,
        StreamPosition, StreamRead,
    };
    use trogon_nats::jetstream::{MockJetStreamKvStore, MockJetStreamPublishMessage};
    use uuid::Uuid;

    use super::*;

    #[derive(Clone, Default)]
    struct ProjectionTestEventStore {
        events: Arc<Mutex<Vec<StreamEvent>>>,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("projection test event store failed")]
    struct ProjectionTestEventStoreError;

    impl ProjectionTestEventStore {
        fn push(&self, stream_id: &str, stream_position: u64, event: CredentialEvent) {
            self.events
                .lock()
                .unwrap()
                .push(stream_event(stream_id, stream_position, event));
        }
    }

    impl StreamRead<str> for ProjectionTestEventStore {
        type Error = ProjectionTestEventStoreError;

        async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
            let start = match request.from {
                ReadFrom::Beginning => 1,
                ReadFrom::Position(position) => position.as_u64(),
            };
            let events = self.events.lock().unwrap();
            let current_position = events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id)
                .map(|event| event.stream_position)
                .max();

            Ok(ReadStreamResponse {
                current_position,
                events: events
                    .iter()
                    .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                    .cloned()
                    .collect(),
            })
        }
    }

    fn integration_id() -> SourceIntegrationId {
        SourceIntegrationId::new("primary").unwrap()
    }

    fn owner_id() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
    }

    fn key() -> RuntimeIntegrationKey {
        RuntimeIntegrationKey::new(SourceKind::Discord, &integration_id())
    }

    fn source_key() -> RuntimeIntegrationKey {
        RuntimeIntegrationKey::for_source(SourceKind::Discord)
    }

    async fn put_bot_token(store: &MockOpenBaoSecretStore, value: &str) -> CredentialRef {
        store
            .put(
                CredentialScope::integration(owner_id(), SourceKind::Discord, integration_id()),
                CredentialKind::BotToken,
                SecretString::new(value).unwrap(),
            )
            .await
            .unwrap()
    }

    async fn put_source_bot_token(store: &MockOpenBaoSecretStore, value: &str) -> CredentialRef {
        store
            .put(
                CredentialScope::source(owner_id(), SourceKind::Discord),
                CredentialKind::BotToken,
                SecretString::new(value).unwrap(),
            )
            .await
            .unwrap()
    }

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).unwrap()
    }

    fn stream_event(stream_id: &str, stream_position: u64, event: CredentialEvent) -> StreamEvent {
        StreamEvent {
            stream_id: stream_id.to_string(),
            event: runtime_event(stream_position, event),
            stream_position: position(stream_position),
            recorded_at: Utc::now(),
        }
    }

    fn runtime_event(id: u64, event: CredentialEvent) -> Event {
        Event {
            id: EventId::new(Uuid::from_u128(id as u128)),
            r#type: event.event_type().unwrap().to_string(),
            content: event.encode().unwrap(),
            headers: Headers::empty(),
        }
    }

    async fn raw_stream(events: impl IntoIterator<Item = Event>) -> MockJetStreamPublishMessage {
        let stream = MockJetStreamPublishMessage::new();
        let events = events.into_iter().collect::<Vec<_>>();
        if events.is_empty() {
            return stream;
        }
        append_stream(
            &stream,
            StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
            None,
            &events,
        )
        .await
        .unwrap();
        stream
    }

    fn write_requested(credential: &CredentialRef) -> CredentialEvent {
        CredentialEvent::WriteRequested {
            credential_id: credential.id().clone(),
            owner_id: owner_id(),
            source: credential.source(),
            kind: credential.kind(),
        }
    }

    fn build_state(events: impl IntoIterator<Item = CredentialEvent>) -> CredentialState {
        events
            .into_iter()
            .try_fold(initial_state(), |state, event| evolve(state, &event))
            .unwrap()
    }

    #[tokio::test]
    async fn active_projection_can_be_built_from_credential_ref() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projection = RuntimeIntegrationProjection::active_from_credential_ref(credential, 7).unwrap();
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections.upsert(projection.clone()).await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        assert_eq!(projection.key(), &key());
        assert_eq!(projection.owner_id(), &owner_id());
        assert_eq!(projection.status(), RuntimeIntegrationStatus::Active);
        assert_eq!(projection.version(), 7);
        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn active_projection_can_be_refreshed_from_replayed_state() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let state = [
            CredentialEvent::WriteRequested {
                credential_id: credential.id().clone(),
                owner_id: owner_id(),
                source: credential.source(),
                kind: credential.kind(),
            },
            CredentialEvent::Activated { metadata },
        ]
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap();
        let projection = RuntimeIntegrationProjection::from_credential_state(&state, 2)
            .unwrap()
            .unwrap();
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections.upsert(projection).await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn refresh_rebuilds_runtime_projection_from_credential_events() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store);

        let report = registry
            .refresh_from_credential_events([
                stream_event("credential-a", 1, write_requested(&credential)),
                stream_event("credential-a", 2, CredentialEvent::Activated { metadata }),
            ])
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 2);
        assert_eq!(report.decoded_events(), 2);
        assert_eq!(report.skipped_events(), 0);
        assert_eq!(report.projected_integrations(), 1);
        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn refresh_rebuilds_runtime_projection_from_persisted_credential_stream() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let stream = MockJetStreamPublishMessage::new();
        append_stream(
            &stream,
            StreamSubject::new("credential-a").unwrap(),
            None,
            &[
                runtime_event(1, write_requested(&credential)),
                runtime_event(2, CredentialEvent::Activated { metadata }),
            ],
        )
        .await
        .unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store);

        let report = registry.refresh_from_credential_stream(&stream, 1).await.unwrap();

        assert_eq!(report.scanned_events(), 2);
        assert_eq!(report.decoded_events(), 2);
        assert_eq!(report.projected_integrations(), 1);
        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn projection_checkpoint_store_loads_default_when_missing() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());

        let checkpoint = checkpoints.load().await.unwrap();

        assert_eq!(checkpoint.last_scanned_sequence(), 0);
        assert_eq!(kv.entry_calls(), vec![RUNTIME_PROJECTION_CHECKPOINT_KEY.to_string()]);
        assert!(kv.create_calls().is_empty());
        assert!(kv.update_calls().is_empty());
    }

    #[tokio::test]
    async fn projection_checkpoint_store_saves_cursor() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());

        checkpoints.save(RuntimeProjectionCheckpoint::new(42)).await.unwrap();

        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, RUNTIME_PROJECTION_CHECKPOINT_KEY);
        assert_eq!(
            decode_projection_checkpoint(&calls[0].1)
                .unwrap()
                .last_scanned_sequence(),
            42
        );
    }

    #[tokio::test]
    async fn checkpointed_refresh_advances_projection_checkpoint_after_success() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let event_store = ProjectionTestEventStore::default();
        event_store.push(credential.id().as_str(), 1, write_requested(&credential));
        event_store.push(
            credential.id().as_str(),
            2,
            CredentialEvent::Activated {
                metadata: metadata.clone(),
            },
        );
        let stream = raw_stream([
            runtime_event(1, write_requested(&credential)),
            runtime_event(2, CredentialEvent::Activated { metadata }),
        ])
        .await;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        kv.enqueue_entry_none();
        let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store);

        let report = registry
            .refresh_from_credential_stream_checkpointed(&stream, &event_store, &checkpoints)
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 2);
        assert_eq!(report.decoded_events(), 2);
        assert_eq!(report.changed_credentials(), 1);
        assert_eq!(report.applied_credentials(), 1);
        assert_eq!(report.projected_integrations(), 1);
        assert_eq!(report.checkpoint_loaded_sequence(), 0);
        assert_eq!(report.checkpoint_advanced_to(), Some(2));
        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            decode_projection_checkpoint(&calls[0].1)
                .unwrap()
                .last_scanned_sequence(),
            2
        );
    }

    #[tokio::test]
    async fn checkpointed_refresh_starts_after_loaded_checkpoint() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let event_store = ProjectionTestEventStore::default();
        event_store.push(credential.id().as_str(), 1, write_requested(&credential));
        event_store.push(
            credential.id().as_str(),
            2,
            CredentialEvent::Activated {
                metadata: metadata.clone(),
            },
        );
        let stream = raw_stream([
            runtime_event(1, write_requested(&credential)),
            runtime_event(2, CredentialEvent::Activated { metadata }),
        ])
        .await;
        let checkpoint = RuntimeProjectionCheckpoint::new(1);
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(
            Bytes::from(encode_projection_checkpoint(checkpoint)),
            3,
            kv::Operation::Put,
        );
        kv.enqueue_entry(
            Bytes::from(encode_projection_checkpoint(checkpoint)),
            4,
            kv::Operation::Put,
        );
        let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());
        let registry = RuntimeCredentialRegistry::default();

        let report = registry
            .refresh_from_credential_stream_checkpointed(&stream, &event_store, &checkpoints)
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 1);
        assert_eq!(report.decoded_events(), 1);
        assert_eq!(report.changed_credentials(), 1);
        assert_eq!(report.applied_credentials(), 1);
        assert_eq!(report.checkpoint_loaded_sequence(), 1);
        assert_eq!(report.checkpoint_advanced_to(), Some(2));
        let calls = kv.update_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            decode_projection_checkpoint(&calls[0].1)
                .unwrap()
                .last_scanned_sequence(),
            2
        );
        assert_eq!(calls[0].2, 4);
    }

    #[tokio::test]
    async fn checkpointed_refresh_does_not_save_checkpoint_when_no_new_events() {
        let event_store = ProjectionTestEventStore::default();
        let stream = raw_stream(Vec::new()).await;
        let checkpoint = RuntimeProjectionCheckpoint::new(2);
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(
            Bytes::from(encode_projection_checkpoint(checkpoint)),
            3,
            kv::Operation::Put,
        );
        let checkpoints = RuntimeProjectionKvCheckpointStore::new(kv.clone());
        let registry = RuntimeCredentialRegistry::default();

        let report = registry
            .refresh_from_credential_stream_checkpointed(&stream, &event_store, &checkpoints)
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 0);
        assert_eq!(report.checkpoint_loaded_sequence(), 2);
        assert_eq!(report.checkpoint_advanced_to(), None);
        assert!(kv.create_calls().is_empty());
        assert!(kv.update_calls().is_empty());
    }

    #[tokio::test]
    async fn refresh_removes_revoked_projection_and_clears_stale_cache() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store.clone());

        registry
            .refresh_from_credential_events([
                stream_event("credential-a", 1, write_requested(&credential)),
                stream_event(
                    "credential-a",
                    2,
                    CredentialEvent::Activated {
                        metadata: metadata.clone(),
                    },
                ),
            ])
            .await
            .unwrap();
        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );

        store.revoke(&credential).await.unwrap();
        let report = registry
            .refresh_from_credential_events([
                stream_event("credential-a", 1, write_requested(&credential)),
                stream_event("credential-a", 2, CredentialEvent::Activated { metadata }),
                stream_event(
                    "credential-a",
                    3,
                    CredentialEvent::Revoked {
                        credential_ref: credential.clone(),
                    },
                ),
            ])
            .await
            .unwrap();

        assert_eq!(report.projected_integrations(), 0);
        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::IntegrationNotFound { .. })
        ));

        registry
            .projections()
            .upsert(projection(RuntimeIntegrationStatus::Active, credential))
            .await;
        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
        ));
    }

    #[tokio::test]
    async fn refresh_projects_source_scoped_active_state() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_source_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store);

        let report = registry
            .refresh_from_credential_events([
                stream_event("credential-a", 1, write_requested(&credential)),
                stream_event("credential-a", 2, CredentialEvent::Activated { metadata }),
            ])
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 2);
        assert_eq!(report.decoded_events(), 2);
        assert_eq!(report.projected_integrations(), 1);
        assert_eq!(
            resolver
                .resolve_plaintext(&source_key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn registry_applies_active_state_incrementally() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store);
        let state = build_state([write_requested(&credential), CredentialEvent::Activated { metadata }]);

        registry.apply_state(&state, position(2)).await.unwrap();

        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn registry_apply_projects_source_scoped_active_state() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_source_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store);
        let state = build_state([write_requested(&credential), CredentialEvent::Activated { metadata }]);

        registry.apply_state(&state, position(2)).await.unwrap();

        assert_eq!(
            resolver
                .resolve_plaintext(&source_key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn registry_applies_revoked_state_incrementally_and_invalidates_cache() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store.clone());
        let active_state = build_state([write_requested(&credential), CredentialEvent::Activated { metadata }]);
        registry.apply_state(&active_state, position(2)).await.unwrap();
        assert_eq!(
            resolver
                .resolve_plaintext(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_str(),
            "Bot token"
        );

        store.revoke(&credential).await.unwrap();
        let revoked_state = evolve(
            active_state,
            &CredentialEvent::Revoked {
                credential_ref: credential.clone(),
            },
        )
        .unwrap();
        registry.apply_state(&revoked_state, position(3)).await.unwrap();

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::IntegrationNotFound { .. })
        ));

        registry
            .projections()
            .upsert(projection(RuntimeIntegrationStatus::Active, credential))
            .await;
        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
        ));
    }

    #[test]
    fn source_scoped_credential_ref_builds_source_runtime_projection_key() {
        let scope = CredentialScope::source(owner_id(), SourceKind::Discord);
        let credential = CredentialRef::new(
            CredentialId::new("openbao:tenant-1:discord:bot_token").unwrap(),
            CredentialVersion::initial(),
            &scope,
            CredentialKind::BotToken,
        );
        let key = RuntimeIntegrationKey::from_credential_ref(&credential).unwrap();

        assert_eq!(key, source_key());
        assert_eq!(key.integration_id(), None);
    }

    fn projection(status: RuntimeIntegrationStatus, credential: CredentialRef) -> RuntimeIntegrationProjection {
        RuntimeIntegrationProjection::new(owner_id(), SourceKind::Discord, integration_id(), status, 1)
            .with_credential(CredentialKind::BotToken, credential)
    }

    #[tokio::test]
    async fn resolves_active_projection_from_secret_store() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        let token = resolver
            .resolve_plaintext(&key(), CredentialKind::BotToken)
            .await
            .unwrap();

        assert_eq!(token.as_str(), "Bot token");
    }

    #[tokio::test]
    async fn disabled_projection_fails_closed_without_reading_secret_store() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Disabled, credential))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::IntegrationNotResolvable {
                status: RuntimeIntegrationStatus::Disabled,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn rotation_uses_new_credential_ref_without_reusing_stale_cache() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot old-token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections.clone(), store.clone());

        assert_eq!(
            resolver
                .resolve(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "Bot old-token"
        );

        let rotated = store
            .rotate(&credential, SecretString::new("Bot new-token").unwrap())
            .await
            .unwrap();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, rotated))
            .await;

        assert_eq!(
            resolver
                .resolve(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "Bot new-token"
        );
    }

    #[tokio::test]
    async fn revoked_store_entry_fails_after_cache_invalidation() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store.clone());

        assert!(
            resolver
                .resolve(&key(), CredentialKind::BotToken)
                .await
                .unwrap()
                .as_plaintext()
                .is_some()
        );

        store.revoke(&credential).await.unwrap();
        resolver.cache().invalidate(&credential).await;

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::SecretStore(SecretStoreError::Unreadable { .. }))
        ));
    }

    #[tokio::test]
    async fn missing_required_credential_fails_closed() {
        let store = MockOpenBaoSecretStore::default();
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(RuntimeIntegrationProjection::new(
                owner_id(),
                SourceKind::Discord,
                integration_id(),
                RuntimeIntegrationStatus::Active,
                1,
            ))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::CredentialMissing {
                kind: CredentialKind::BotToken,
                ..
            })
        ));
    }
}
