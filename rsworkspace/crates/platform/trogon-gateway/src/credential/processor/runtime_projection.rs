#![allow(dead_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_nats::jetstream::{self, context, kv};
use buffa::Message as _;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram};
use tokio::sync::Mutex;
use tracing::{error, info};
use trogon_decider_runtime::{
    EventDecodeOutcome, InvalidStreamPositionError, ReadFrom, ReadStreamRequest, StreamEvent, StreamPosition,
    StreamRead,
};
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamGetRawMessage, JetStreamGetStreamInfo,
    JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry,
    is_create_key_value_already_exists,
};
use trogon_std::SecretString;
use trogonai_proto::gateway::credentials::checkpoints_v1 as proto;
use trogonai_proto::gateway::credentials::{
    CredentialEventCase, CredentialEventPayloadError, CredentialStateSnapshotCase, state_v1, v1,
};

use crate::credential::commands::domain::{
    CredentialId, CredentialKind, CredentialOwnerId, CredentialRef, RuntimeDeliveryDenied, RuntimeDeliveryPolicy,
    RuntimeDeliveryRequest, SourceKind,
};
use crate::credential::processor::event_stream::{CredentialEventStreamReadError, read_credential_event_stream};
use crate::credential::proto::{
    CredentialProtoDecodeError, active_credential_ref, decode_credential_metadata, decode_destroy_failed,
    decode_destroy_requested, decode_destroyed, decode_destroyed_state, decode_message_field, decode_revoked,
    decode_revoked_state, decode_rotated, decode_rotation_failed, decode_rotation_requested, decode_write_failed,
    decode_write_requested,
};
use crate::credential::{CredentialEvolveError, evolve, initial_state};
use crate::secret_store::{SecretMaterial, SecretStoreError, SecretStoreGet};
use crate::source_integration_id::{SourceIntegrationId, SourceIntegrationIdError};

const RUNTIME_PROJECTION_CHECKPOINT_KEY: &str = "v1.runtime-projection";
pub(crate) const CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINT_BUCKET: &str =
    "GATEWAY_CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINTS";
const RUNTIME_PROJECTION_METER_NAME: &str = "trogon-gateway";

#[derive(Debug)]
struct RuntimeProjectionMetrics {
    revocation_latency: Histogram<f64>,
    cache_hits: Counter<u64>,
    cache_misses: Counter<u64>,
    delivery_denials: Counter<u64>,
    resolve_failures: Counter<u64>,
}

impl RuntimeProjectionMetrics {
    fn new() -> Self {
        let meter = trogon_telemetry::meter(RUNTIME_PROJECTION_METER_NAME);
        Self {
            revocation_latency: meter
                .f64_histogram("gateway.credential.revocation.latency")
                .with_description(
                    "Time from a credential revocation event's recorded timestamp to runtime cache invalidation.",
                )
                .with_unit("s")
                .build(),
            cache_hits: meter
                .u64_counter("gateway.credential.cache.hits")
                .with_description("Credential resolutions served from the runtime cache.")
                .build(),
            cache_misses: meter
                .u64_counter("gateway.credential.cache.misses")
                .with_description("Credential resolutions that had to read the secret store.")
                .build(),
            delivery_denials: meter
                .u64_counter("gateway.credential.delivery.denied")
                .with_description("Credential resolutions refused by the credential's runtime delivery policy.")
                .build(),
            resolve_failures: meter
                .u64_counter("gateway.credential.resolve.failures")
                .with_description("Credential resolutions that failed to read material from the secret store.")
                .build(),
        }
    }

    fn record_revocation_latency(&self, revoked_recorded_at: DateTime<Utc>) {
        self.revocation_latency
            .record(revocation_latency_seconds(revoked_recorded_at, Utc::now()), &[]);
    }

    fn record_cache_hit(&self, key: &RuntimeIntegrationKey) {
        self.cache_hits.add(1, &resolution_attributes(key));
    }

    fn record_cache_miss(&self, key: &RuntimeIntegrationKey) {
        self.cache_misses.add(1, &resolution_attributes(key));
    }

    /// The denial reason is a label so a spike can be attributed without
    /// reading logs. The rejected host is not: it is attacker-controlled, and
    /// an unbounded label set is a cardinality bomb.
    fn record_delivery_denial(&self, key: &RuntimeIntegrationKey, denied: &RuntimeDeliveryDenied) {
        let mut attributes = resolution_attributes(key);
        attributes.push(KeyValue::new("reason", denial_reason(denied)));
        self.delivery_denials.add(1, &attributes);
    }

    fn record_resolve_failure(&self, key: &RuntimeIntegrationKey) {
        self.resolve_failures.add(1, &resolution_attributes(key));
    }
}

fn resolution_attributes(key: &RuntimeIntegrationKey) -> Vec<KeyValue> {
    vec![KeyValue::new("source", key.source().as_str().to_string())]
}

fn denial_reason(denied: &RuntimeDeliveryDenied) -> &'static str {
    match denied {
        RuntimeDeliveryDenied::RuntimeService { .. } => "runtime_service",
        RuntimeDeliveryDenied::Host { .. } => "host",
        RuntimeDeliveryDenied::InjectionLocation { .. } => "injection_location",
    }
}

static RUNTIME_PROJECTION_METRICS: OnceLock<RuntimeProjectionMetrics> = OnceLock::new();

fn runtime_projection_metrics() -> &'static RuntimeProjectionMetrics {
    RUNTIME_PROJECTION_METRICS.get_or_init(RuntimeProjectionMetrics::new)
}

fn revocation_latency_seconds(recorded_at: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    (now - recorded_at).to_std().unwrap_or(Duration::ZERO).as_secs_f64()
}

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

/// `owner_id` is the project id: ADR#0046 collapses workspace-shaped fields
/// into the project, so the delivery policy's `workspace_id` is this field rather
/// than a second one beside it.
#[derive(Clone, Debug)]
pub struct RuntimeIntegrationProjection {
    key: RuntimeIntegrationKey,
    owner_id: CredentialOwnerId,
    status: RuntimeIntegrationStatus,
    version: u64,
    credentials: BTreeMap<CredentialKind, CredentialRef>,
    delivery_policy: RuntimeDeliveryPolicy,
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
            delivery_policy: RuntimeDeliveryPolicy::default(),
        }
    }

    pub fn with_credential(mut self, kind: CredentialKind, credential: CredentialRef) -> Self {
        self.credentials.insert(kind, credential);
        self
    }

    pub fn with_delivery_policy(mut self, delivery_policy: RuntimeDeliveryPolicy) -> Self {
        self.delivery_policy = delivery_policy;
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
            delivery_policy: RuntimeDeliveryPolicy::default(),
        })
    }

    pub fn from_credential_state(
        state: &state_v1::CredentialStateSnapshot,
        version: u64,
    ) -> Result<Option<Self>, RuntimeProjectionBuildError> {
        match state.state.as_ref() {
            Some(CredentialStateSnapshotCase::Active(active)) => {
                let credential_ref =
                    active_credential_ref(active).map_err(RuntimeProjectionBuildError::InvalidState)?;
                active_runtime_projection(credential_ref, version)
            }
            Some(CredentialStateSnapshotCase::RotationPending(rotation)) => {
                let active = decode_message_field("rotation_pending.active", &rotation.active)
                    .map_err(RuntimeProjectionBuildError::InvalidState)?;
                let credential_ref =
                    active_credential_ref(active).map_err(RuntimeProjectionBuildError::InvalidState)?;
                active_runtime_projection(credential_ref, version)
            }
            None
            | Some(
                CredentialStateSnapshotCase::Missing(_)
                | CredentialStateSnapshotCase::PendingWrite(_)
                | CredentialStateSnapshotCase::WriteFailed(_)
                | CredentialStateSnapshotCase::Revoked(_)
                | CredentialStateSnapshotCase::DestroyRequested(_)
                | CredentialStateSnapshotCase::Destroyed(_)
                | CredentialStateSnapshotCase::CleanupFailed(_),
            ) => Ok(None),
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

    pub fn delivery_policy(&self) -> &RuntimeDeliveryPolicy {
        &self.delivery_policy
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
        delivery_policy: RuntimeDeliveryPolicy::default(),
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
    #[error("persisted credential state is invalid: {0}")]
    InvalidState(#[source] CredentialProtoDecodeError),
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
    #[error("credential event is missing its event case")]
    MissingEvent,
    #[error("credential event is invalid: {source}")]
    InvalidEvent {
        #[source]
        source: CredentialProtoDecodeError,
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
    InvalidStreamPositionError {
        #[source]
        source: InvalidStreamPositionError,
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
        source: CredentialEventStreamReadError,
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
        state: &state_v1::CredentialStateSnapshot,
        stream_position: StreamPosition,
    ) -> Result<(), RuntimeProjectionRefreshError> {
        if let Some(projection) = RuntimeIntegrationProjection::from_credential_state(state, stream_position.as_u64())
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?
        {
            self.projections.merge(projection).await?;
            self.cache.clear().await;
            return Ok(());
        }

        if let Some(CredentialStateSnapshotCase::Revoked(revoked)) = state.state.as_ref() {
            let credential_ref = decode_revoked_state(revoked)
                .map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })?;
            self.remove_credential_ref(&credential_ref).await?;
        }

        if let Some(CredentialStateSnapshotCase::Destroyed(destroyed)) = state.state.as_ref() {
            let credential_ref = decode_destroyed_state(destroyed)
                .map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })?;
            self.remove_credential_ref(&credential_ref).await?;
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
    let events = read_credential_event_stream(stream, from_sequence)
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
    let events = read_credential_event_stream(event_stream, from_sequence)
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
    let mut streams: BTreeMap<String, Vec<(u64, v1::CredentialEvent)>> = BTreeMap::new();

    for event in events {
        report.scanned_events += 1;
        let stream_id = event.stream_id().to_string();
        let stream_position = event.stream_position.as_u64();
        match event
            .decode::<v1::CredentialEvent>()
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
    let mut revoked_recorded_at = BTreeMap::<CredentialId, DateTime<Utc>>::new();

    for event in events {
        report.scanned_events += 1;
        let stream_position = event.stream_position.as_u64();
        let recorded_at = event.recorded_at;
        report.checkpoint_advanced_to = Some(report.checkpoint_advanced_to.unwrap_or(0).max(stream_position));
        match event
            .decode::<v1::CredentialEvent>()
            .map_err(|source| RuntimeProjectionRefreshError::DecodeEvent { source })?
        {
            EventDecodeOutcome::Decoded(event) => {
                report.decoded_events += 1;
                let case = event
                    .event
                    .as_ref()
                    .ok_or(RuntimeProjectionRefreshError::MissingEvent)?;
                let credential_id = event_credential_id(case)
                    .map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })?;
                if matches!(case, CredentialEventCase::Revoked(_)) {
                    revoked_recorded_at.insert(credential_id.clone(), recorded_at);
                }
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
        let revoked_event_recorded_at = revoked_recorded_at.get(&credential_id).copied();
        apply_state_to_projection(
            projections,
            cache,
            &state,
            position(version)?,
            revoked_event_recorded_at,
        )
        .await?;
        report.applied_credentials += 1;
    }
    report.projected_integrations = projections.len().await;

    Ok(report)
}

async fn load_credential_state<EventStore>(
    event_store: &EventStore,
    credential_id: &CredentialId,
) -> Result<state_v1::CredentialStateSnapshot, RuntimeProjectionRefreshError>
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
            .decode::<v1::CredentialEvent>()
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
    state: &state_v1::CredentialStateSnapshot,
    stream_position: StreamPosition,
    revoked_event_recorded_at: Option<DateTime<Utc>>,
) -> Result<(), RuntimeProjectionRefreshError> {
    if let Some(projection) = RuntimeIntegrationProjection::from_credential_state(state, stream_position.as_u64())
        .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?
    {
        projections.merge(projection).await?;
        cache.clear().await;
        return Ok(());
    }

    if let Some(CredentialStateSnapshotCase::Revoked(revoked)) = state.state.as_ref() {
        let credential_ref =
            decode_revoked_state(revoked).map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })?;
        cache.invalidate(&credential_ref).await;
        let key = RuntimeIntegrationKey::from_credential_ref(&credential_ref)
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?;
        projections.remove_credential(&key, credential_ref.kind()).await;
        if let Some(recorded_at) = revoked_event_recorded_at {
            runtime_projection_metrics().record_revocation_latency(recorded_at);
        }
    }

    if let Some(CredentialStateSnapshotCase::Destroyed(destroyed)) = state.state.as_ref() {
        let credential_ref = decode_destroyed_state(destroyed)
            .map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })?;
        cache.invalidate(&credential_ref).await;
        let key = RuntimeIntegrationKey::from_credential_ref(&credential_ref)
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?;
        projections.remove_credential(&key, credential_ref.kind()).await;
    }
    Ok(())
}

fn event_credential_id(event: &CredentialEventCase) -> Result<CredentialId, CredentialProtoDecodeError> {
    match event {
        CredentialEventCase::WriteRequested(inner) => Ok(decode_write_requested(inner)?.0),
        CredentialEventCase::WriteFailed(inner) => Ok(decode_write_failed(inner)?.0),
        CredentialEventCase::Activated(inner) => {
            let metadata = decode_message_field("event.metadata", &inner.metadata)?;
            Ok(decode_credential_metadata("event.metadata", metadata)?
                .reference()
                .id()
                .clone())
        }
        CredentialEventCase::RotationRequested(inner) => Ok(decode_rotation_requested(inner)?.id().clone()),
        CredentialEventCase::RotationFailed(inner) => Ok(decode_rotation_failed(inner)?.0.id().clone()),
        CredentialEventCase::Revoked(inner) => Ok(decode_revoked(inner)?.id().clone()),
        CredentialEventCase::Rotated(inner) => Ok(decode_rotated(inner)?.0.id().clone()),
        CredentialEventCase::DestroyRequested(inner) => Ok(decode_destroy_requested(inner)?.0.id().clone()),
        CredentialEventCase::Destroyed(inner) => Ok(decode_destroyed(inner)?.id().clone()),
        CredentialEventCase::DestroyFailed(inner) => Ok(decode_destroy_failed(inner)?.0.id().clone()),
    }
}

fn position(value: u64) -> Result<StreamPosition, RuntimeProjectionRefreshError> {
    StreamPosition::try_new(value)
        .map_err(|source| RuntimeProjectionRefreshError::InvalidStreamPositionError { source })
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeCredentialCachePolicy {
    ttl: Duration,
    jitter: Duration,
}

impl RuntimeCredentialCachePolicy {
    pub fn new(ttl: Duration, jitter: Duration) -> Result<Self, RuntimeCredentialCachePolicyError> {
        if ttl.is_zero() {
            return Err(RuntimeCredentialCachePolicyError::ZeroTtl);
        }
        if jitter > ttl {
            return Err(RuntimeCredentialCachePolicyError::JitterExceedsTtl { ttl, jitter });
        }
        Ok(Self { ttl, jitter })
    }

    pub fn ttl(self) -> Duration {
        self.ttl
    }

    pub fn jitter(self) -> Duration {
        self.jitter
    }
}

impl Default for RuntimeCredentialCachePolicy {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(300),
            jitter: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeCredentialCachePolicyError {
    #[error("runtime credential cache ttl must be greater than zero")]
    ZeroTtl,
    #[error("runtime credential cache jitter {jitter:?} must not exceed ttl {ttl:?}")]
    JitterExceedsTtl { ttl: Duration, jitter: Duration },
}

struct RuntimeCredentialCacheEntry {
    material: SecretMaterial,
    expires_at: Instant,
}

#[derive(Clone, Default)]
pub struct RuntimeCredentialCache {
    entries: Arc<Mutex<BTreeMap<CredentialRef, RuntimeCredentialCacheEntry>>>,
    policy: RuntimeCredentialCachePolicy,
}

impl RuntimeCredentialCache {
    pub fn with_policy(policy: RuntimeCredentialCachePolicy) -> Self {
        Self {
            entries: Arc::new(Mutex::new(BTreeMap::new())),
            policy,
        }
    }

    async fn get(&self, credential: &CredentialRef) -> Option<SecretMaterial> {
        self.get_at(credential, Instant::now()).await
    }

    async fn get_at(&self, credential: &CredentialRef, now: Instant) -> Option<SecretMaterial> {
        let mut entries = self.entries.lock().await;
        match entries.get(credential) {
            Some(entry) if entry.expires_at > now => Some(entry.material.clone()),
            Some(_) => {
                entries.remove(credential);
                None
            }
            None => None,
        }
    }

    pub fn policy(&self) -> RuntimeCredentialCachePolicy {
        self.policy
    }

    async fn put(&self, credential: CredentialRef, material: SecretMaterial) {
        self.put_at(credential, material, Instant::now()).await;
    }

    async fn put_at(&self, credential: CredentialRef, material: SecretMaterial, now: Instant) {
        self.put_at_with_ttl(credential, material, self.policy.ttl, now).await;
    }

    /// Inserts under a caller-supplied ttl, used to honour a credential's
    /// `RuntimeDeliveryPolicy` cache override.
    ///
    /// The override can only shorten, so the jitter is clamped to the shortened
    /// window rather than allowed to saturate it away.
    async fn put_with_ttl(&self, credential: CredentialRef, material: SecretMaterial, ttl: Duration) {
        self.put_at_with_ttl(credential, material, ttl, Instant::now()).await;
    }

    async fn put_at_with_ttl(&self, credential: CredentialRef, material: SecretMaterial, ttl: Duration, now: Instant) {
        let expires_at = now + self.expiry_offset_within(&credential, ttl);
        self.entries
            .lock()
            .await
            .insert(credential, RuntimeCredentialCacheEntry { material, expires_at });
    }

    fn expiry_offset_within(&self, credential: &CredentialRef, ttl: Duration) -> Duration {
        ttl.saturating_sub(self.key_jitter_within(credential, ttl))
    }

    fn key_jitter_within(&self, credential: &CredentialRef, ttl: Duration) -> Duration {
        let jitter = self.policy.jitter.min(ttl);
        if jitter.is_zero() {
            return Duration::ZERO;
        }
        let mut hasher = DefaultHasher::new();
        credential.to_string().hash(&mut hasher);
        let hash = u128::from(hasher.finish());
        let jitter_nanos = jitter.as_nanos();
        let offset_nanos = hash % jitter_nanos;
        Duration::from_nanos(u64::try_from(offset_nanos).unwrap_or(u64::MAX))
    }

    pub async fn invalidate(&self, credential: &CredentialRef) {
        self.entries.lock().await.remove(credential);
    }

    pub async fn clear(&self) {
        self.entries.lock().await.clear();
    }

    #[cfg(test)]
    async fn expires_at(&self, credential: &CredentialRef) -> Option<Instant> {
        self.entries.lock().await.get(credential).map(|entry| entry.expires_at)
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
        self.resolve_for(key, kind, &RuntimeDeliveryRequest::new()).await
    }

    /// Resolves under an explicit delivery request so the projection's policy
    /// can be enforced.
    ///
    /// `resolve` is this with an empty request, which a default (unrestricted)
    /// policy permits and a configured policy denies. The deny happens before
    /// the store is touched, so a denied caller cannot warm the cache or
    /// observe a difference between a present and an absent credential.
    pub async fn resolve_for(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
        request: &RuntimeDeliveryRequest<'_>,
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

        projection.delivery_policy().permits(request).map_err(|denied| {
            runtime_projection_metrics().record_delivery_denial(key, &denied);
            RuntimeCredentialError::DeliveryDenied {
                key: key.clone(),
                kind,
                denied,
            }
        })?;

        let credential = projection
            .credential(kind)
            .ok_or_else(|| RuntimeCredentialError::CredentialMissing { key: key.clone(), kind })?;

        if let Some(material) = self.cache.get(credential).await {
            runtime_projection_metrics().record_cache_hit(key);
            return Ok(material);
        }

        runtime_projection_metrics().record_cache_miss(key);
        let material = self.store.get(credential).await.inspect_err(|_| {
            runtime_projection_metrics().record_resolve_failure(key);
        })?;
        let ttl = projection
            .delivery_policy()
            .effective_cache_ttl(self.cache.policy().ttl());
        self.cache.put_with_ttl(credential.clone(), material.clone(), ttl).await;
        Ok(material)
    }

    pub async fn resolve_plaintext(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
    ) -> Result<SecretString, RuntimeCredentialError> {
        self.resolve_plaintext_for(key, kind, &RuntimeDeliveryRequest::new())
            .await
    }

    pub async fn resolve_plaintext_for(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
        request: &RuntimeDeliveryRequest<'_>,
    ) -> Result<SecretString, RuntimeCredentialError> {
        match self.resolve_for(key, kind, request).await? {
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
    #[error("runtime credential delivery denied: {key} {kind}: {denied}")]
    DeliveryDenied {
        key: RuntimeIntegrationKey,
        kind: CredentialKind,
        #[source]
        denied: RuntimeDeliveryDenied,
    },
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
}

impl RuntimeCredentialError {
    pub fn is_secret_store_error(&self) -> bool {
        matches!(self, Self::SecretStore(_))
    }

    pub fn is_delivery_denied(&self) -> bool {
        matches!(self, Self::DeliveryDenied { .. })
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

    use crate::credential::commands::domain::{
        AllowedHosts, AllowedRuntimeServices, CredentialScope, CredentialVersion, InjectionLocation,
        InjectionLocations, RuntimeServiceId,
    };
    use crate::credential::proto::{
        activated_to_proto, destroy_requested_to_proto, destroyed_to_proto, revoked_to_proto, write_requested_to_proto,
    };
    use crate::secret_store::{
        MockOpenBaoSecretStore, SecretDestroyReason, SecretStoreDestroy, SecretStoreMetadata, SecretStorePut,
        SecretStoreRevoke, SecretStoreRotate, SecretVerifier,
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
        fn push(&self, stream_id: &str, stream_position: u64, event: v1::CredentialEvent) {
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

    fn stream_event(stream_id: &str, stream_position: u64, event: v1::CredentialEvent) -> StreamEvent {
        StreamEvent {
            stream_id: stream_id.to_string(),
            event: runtime_event(stream_position, event),
            stream_position: position(stream_position),
            recorded_at: Utc::now(),
        }
    }

    fn runtime_event(id: u64, event: v1::CredentialEvent) -> Event {
        Event {
            id: EventId::new(Uuid::from_u128(id as u128)),
            r#type: EventType::event_type(&event).unwrap().to_string(),
            content: EventEncode::encode(&event).unwrap(),
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

    fn write_requested(credential: &CredentialRef) -> v1::CredentialEvent {
        v1::CredentialEvent {
            event: Some(
                write_requested_to_proto(credential.id(), &owner_id(), credential.source(), credential.kind()).into(),
            ),
        }
    }

    fn build_state(events: impl IntoIterator<Item = v1::CredentialEvent>) -> state_v1::CredentialStateSnapshot {
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
            write_requested(&credential),
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
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
                stream_event(
                    "credential-a",
                    2,
                    v1::CredentialEvent {
                        event: Some(activated_to_proto(&metadata).into()),
                    },
                ),
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
                runtime_event(
                    2,
                    v1::CredentialEvent {
                        event: Some(activated_to_proto(&metadata).into()),
                    },
                ),
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
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        );
        let stream = raw_stream([
            runtime_event(1, write_requested(&credential)),
            runtime_event(
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
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
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        );
        let stream = raw_stream([
            runtime_event(1, write_requested(&credential)),
            runtime_event(
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
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
                    v1::CredentialEvent {
                        event: Some(activated_to_proto(&metadata).into()),
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
                stream_event(
                    "credential-a",
                    2,
                    v1::CredentialEvent {
                        event: Some(activated_to_proto(&metadata).into()),
                    },
                ),
                stream_event(
                    "credential-a",
                    3,
                    v1::CredentialEvent {
                        event: Some(revoked_to_proto(&credential).into()),
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
                stream_event(
                    "credential-a",
                    2,
                    v1::CredentialEvent {
                        event: Some(activated_to_proto(&metadata).into()),
                    },
                ),
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
        let state = build_state([
            write_requested(&credential),
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ]);

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
        let state = build_state([
            write_requested(&credential),
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ]);

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
        let active_state = build_state([
            write_requested(&credential),
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ]);
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
            &v1::CredentialEvent {
                event: Some(revoked_to_proto(&credential).into()),
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

    #[tokio::test]
    async fn registry_applies_destroyed_state_incrementally_and_invalidates_cache() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store.clone());
        let active_state = build_state([
            write_requested(&credential),
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ]);
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
            &v1::CredentialEvent {
                event: Some(revoked_to_proto(&credential).into()),
            },
        )
        .unwrap();
        registry.apply_state(&revoked_state, position(3)).await.unwrap();

        let reason = SecretDestroyReason::new("credential lifecycle cleanup").unwrap();
        let destroy_requested_state = evolve(
            revoked_state,
            &v1::CredentialEvent {
                event: Some(destroy_requested_to_proto(&credential, &reason).into()),
            },
        )
        .unwrap();
        registry
            .apply_state(&destroy_requested_state, position(4))
            .await
            .unwrap();

        store.destroy(&credential, &reason).await.unwrap();
        let destroyed_state = evolve(
            destroy_requested_state,
            &v1::CredentialEvent {
                event: Some(destroyed_to_proto(&credential).into()),
            },
        )
        .unwrap();
        registry.apply_state(&destroyed_state, position(5)).await.unwrap();

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

    /// Collects everything the tracing layer writes so a test can assert what
    /// never reached it.
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl CapturedLogs {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    use tracing_subscriber::util::SubscriberInitExt;

    const LEAK_CANARY: &str = "canary-plaintext-must-never-be-logged";

    #[tokio::test]
    async fn resolving_and_rotating_never_writes_plaintext_to_the_tracing_layer() {
        let logs = CapturedLogs::default();
        let _guard = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .set_default();

        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, LEAK_CANARY).await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()))
            .await;
        let resolver = RuntimeCredentialResolver::new(projections.clone(), store.clone());

        let material = resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();
        assert_eq!(material.as_plaintext().unwrap().as_str(), LEAK_CANARY);

        let rotated = store
            .rotate(&credential, SecretString::new(LEAK_CANARY).unwrap())
            .await
            .unwrap();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, rotated.clone()))
            .await;
        resolver.cache().invalidate(&credential).await;
        resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();

        store.revoke(&rotated).await.unwrap();
        resolver.cache().invalidate(&rotated).await;
        let revoked = resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap_err();

        tracing::error!(error = ?revoked, "captured resolve failure");
        tracing::error!(error = %revoked, "captured resolve failure");
        tracing::info!(material = ?material, credential = %rotated, "captured resolved material");
        tracing::info!(projection = ?projections.get(&key()).await, "captured projection");

        let contents = logs.contents();
        assert!(contents.contains("captured resolve failure"));
        assert!(contents.contains("captured resolved material"));
        assert!(
            !contents.contains(LEAK_CANARY),
            "plaintext leaked into logs: {contents}"
        );
    }

    #[test]
    fn every_carrier_of_plaintext_redacts_its_debug_rendering() {
        let material = SecretMaterial::Plaintext(SecretString::new(LEAK_CANARY).unwrap());
        let verifier = SecretMaterial::Verifier(SecretVerifier::new(LEAK_CANARY).unwrap());

        assert!(!format!("{material:?}").contains(LEAK_CANARY));
        assert!(!format!("{verifier:?}").contains(LEAK_CANARY));
    }

    #[tokio::test]
    async fn a_delivery_denial_message_carries_no_plaintext() {
        let (resolver, _) = restricted_resolver(
            RuntimeDeliveryPolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap()),
        )
        .await;

        let denied = resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeDeliveryRequest::new().to_host("api.evil.net"),
            )
            .await
            .unwrap_err();

        assert!(denied.to_string().contains("api.evil.net"));
        assert!(!denied.to_string().contains("Bot token"));
        assert!(!format!("{denied:?}").contains("Bot token"));
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

    async fn restricted_resolver(
        policy: RuntimeDeliveryPolicy,
    ) -> (RuntimeCredentialResolver<MockOpenBaoSecretStore>, CredentialRef) {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(projection(RuntimeIntegrationStatus::Active, credential.clone()).with_delivery_policy(policy))
            .await;
        (RuntimeCredentialResolver::new(projections, store), credential)
    }

    #[test]
    fn a_wildcard_host_covers_one_label_and_not_the_apex() {
        let hosts = AllowedHosts::only(["*.example.com", "trogonai.dev"]).unwrap();

        assert!(hosts.permits(Some("api.example.com")));
        assert!(hosts.permits(Some("API.Example.COM.")));
        assert!(!hosts.permits(Some("example.com")));
        assert!(!hosts.permits(Some("a.b.example.com")));
        assert!(hosts.permits(Some("trogonai.dev")));
        assert!(!hosts.permits(Some("api.trogonai.dev")));
        assert!(!hosts.permits(None));
    }

    #[tokio::test]
    async fn unconfigured_delivery_policy_keeps_shipped_source_paths_resolving() {
        let (resolver, _) = restricted_resolver(RuntimeDeliveryPolicy::default()).await;

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
    async fn host_restriction_denies_an_unlisted_host_and_permits_a_listed_one() {
        let (resolver, _) = restricted_resolver(
            RuntimeDeliveryPolicy::default().with_allowed_hosts(AllowedHosts::only(["*.example.com"]).unwrap()),
        )
        .await;

        let denied = resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeDeliveryRequest::new().to_host("api.evil.net"),
            )
            .await
            .unwrap_err();
        assert!(denied.is_delivery_denied());

        assert!(
            resolver
                .resolve_for(
                    &key(),
                    CredentialKind::BotToken,
                    &RuntimeDeliveryRequest::new().to_host("api.example.com:443"),
                )
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn host_restriction_denies_the_unqualified_resolve_path() {
        let (resolver, _) = restricted_resolver(
            RuntimeDeliveryPolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap()),
        )
        .await;

        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::DeliveryDenied {
                denied: RuntimeDeliveryDenied::Host { .. },
                ..
            })
        ));
    }

    #[tokio::test]
    async fn unauthorized_runtime_service_cannot_resolve() {
        let (resolver, _) = restricted_resolver(
            RuntimeDeliveryPolicy::default()
                .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap()),
        )
        .await;
        let intruder = RuntimeServiceId::new("some-other-worker").unwrap();
        let allowed = RuntimeServiceId::new("trogon-gateway").unwrap();

        assert!(matches!(
            resolver
                .resolve_for(
                    &key(),
                    CredentialKind::BotToken,
                    &RuntimeDeliveryRequest::new().by_runtime_service(&intruder),
                )
                .await,
            Err(RuntimeCredentialError::DeliveryDenied {
                denied: RuntimeDeliveryDenied::RuntimeService { .. },
                ..
            })
        ));
        assert!(
            resolver
                .resolve_for(
                    &key(),
                    CredentialKind::BotToken,
                    &RuntimeDeliveryRequest::new().by_runtime_service(&allowed),
                )
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn injection_location_restriction_denies_a_query_parameter_placement() {
        let header = InjectionLocation::header("authorization").unwrap();
        let query = InjectionLocation::query_parameter("access_token").unwrap();
        let (resolver, _) = restricted_resolver(
            RuntimeDeliveryPolicy::default().with_injection_locations(InjectionLocations::new([header.clone()])),
        )
        .await;

        assert!(matches!(
            resolver
                .resolve_for(
                    &key(),
                    CredentialKind::BotToken,
                    &RuntimeDeliveryRequest::new().at_injection_location(&query),
                )
                .await,
            Err(RuntimeCredentialError::DeliveryDenied {
                denied: RuntimeDeliveryDenied::InjectionLocation { .. },
                ..
            })
        ));
        assert!(
            resolver
                .resolve_for(
                    &key(),
                    CredentialKind::BotToken,
                    &RuntimeDeliveryRequest::new().at_injection_location(&header),
                )
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_denied_request_does_not_warm_the_cache() {
        let (resolver, credential) = restricted_resolver(
            RuntimeDeliveryPolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap()),
        )
        .await;

        let _ = resolver
            .resolve_for(
                &key(),
                CredentialKind::BotToken,
                &RuntimeDeliveryRequest::new().to_host("api.evil.net"),
            )
            .await;

        assert!(resolver.cache().expires_at(&credential).await.is_none());
    }

    #[tokio::test]
    async fn a_denial_does_not_reveal_whether_the_credential_exists() {
        let policy =
            RuntimeDeliveryPolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap());
        let store = MockOpenBaoSecretStore::default();
        let projections = InMemoryRuntimeProjectionRepository::default();
        projections
            .upsert(
                RuntimeIntegrationProjection::new(
                    owner_id(),
                    SourceKind::Discord,
                    integration_id(),
                    RuntimeIntegrationStatus::Active,
                    1,
                )
                .with_delivery_policy(policy),
            )
            .await;
        let resolver = RuntimeCredentialResolver::new(projections, store);

        assert!(matches!(
            resolver
                .resolve_for(
                    &key(),
                    CredentialKind::BotToken,
                    &RuntimeDeliveryRequest::new().to_host("api.evil.net"),
                )
                .await,
            Err(RuntimeCredentialError::DeliveryDenied { .. })
        ));
    }

    #[tokio::test]
    async fn cache_ttl_override_shortens_the_cached_window() {
        let (default_resolver, default_credential) = restricted_resolver(RuntimeDeliveryPolicy::default()).await;
        let (short_resolver, short_credential) = restricted_resolver(
            RuntimeDeliveryPolicy::default()
                .with_cache_ttl_override(Duration::from_secs(5))
                .unwrap(),
        )
        .await;

        default_resolver
            .resolve(&key(), CredentialKind::BotToken)
            .await
            .unwrap();
        short_resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();

        let now = Instant::now();
        let default_remaining = default_resolver
            .cache()
            .expires_at(&default_credential)
            .await
            .unwrap()
            .saturating_duration_since(now);
        let short_remaining = short_resolver
            .cache()
            .expires_at(&short_credential)
            .await
            .unwrap()
            .saturating_duration_since(now);

        assert!(short_remaining <= Duration::from_secs(5));
        assert!(short_remaining < default_remaining);
    }

    #[tokio::test]
    async fn cache_ttl_override_may_not_extend_the_cached_window() {
        let (resolver, credential) = restricted_resolver(
            RuntimeDeliveryPolicy::default()
                .with_cache_ttl_override(Duration::from_secs(86_400))
                .unwrap(),
        )
        .await;

        resolver.resolve(&key(), CredentialKind::BotToken).await.unwrap();

        let remaining = resolver
            .cache()
            .expires_at(&credential)
            .await
            .unwrap()
            .saturating_duration_since(Instant::now());
        assert!(remaining <= RuntimeCredentialCachePolicy::default().ttl());
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

    fn cache_credential(id: &str) -> CredentialRef {
        CredentialRef::new(
            CredentialId::new(id).unwrap(),
            CredentialVersion::initial(),
            &CredentialScope::integration(owner_id(), SourceKind::Discord, integration_id()),
            CredentialKind::BotToken,
        )
    }

    fn cache_secret(value: &str) -> SecretMaterial {
        SecretMaterial::Plaintext(SecretString::new(value).unwrap())
    }

    #[tokio::test]
    async fn cache_returns_fresh_entry_within_ttl() {
        let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::from_secs(5)).unwrap();
        let cache = RuntimeCredentialCache::with_policy(policy);
        let credential = cache_credential("credential-fresh");
        let now = Instant::now();

        cache.put_at(credential.clone(), cache_secret("token"), now).await;

        let material = cache.get_at(&credential, now + Duration::from_secs(1)).await.unwrap();
        assert_eq!(material.as_plaintext().unwrap().as_str(), "token");
    }

    #[tokio::test]
    async fn cache_evicts_entry_past_its_jittered_deadline() {
        let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::from_secs(5)).unwrap();
        let cache = RuntimeCredentialCache::with_policy(policy);
        let credential = cache_credential("credential-expiring");
        let now = Instant::now();

        cache.put_at(credential.clone(), cache_secret("token"), now).await;
        let expires_at = cache.expires_at(&credential).await.unwrap();

        assert!(
            cache
                .get_at(&credential, expires_at + Duration::from_nanos(1))
                .await
                .is_none()
        );
        assert!(cache.expires_at(&credential).await.is_none());
    }

    #[tokio::test]
    async fn cache_invalidate_removes_entry_explicitly() {
        let cache = RuntimeCredentialCache::default();
        let credential = cache_credential("credential-invalidate");

        cache.put(credential.clone(), cache_secret("token")).await;
        cache.invalidate(&credential).await;

        assert!(cache.get(&credential).await.is_none());
    }

    #[test]
    fn cache_policy_rejects_zero_ttl() {
        assert!(matches!(
            RuntimeCredentialCachePolicy::new(Duration::ZERO, Duration::ZERO),
            Err(RuntimeCredentialCachePolicyError::ZeroTtl)
        ));
    }

    #[test]
    fn cache_policy_rejects_jitter_greater_than_ttl() {
        assert!(matches!(
            RuntimeCredentialCachePolicy::new(Duration::from_secs(10), Duration::from_secs(11)),
            Err(RuntimeCredentialCachePolicyError::JitterExceedsTtl { .. })
        ));
    }

    #[tokio::test]
    async fn cache_applies_deterministic_per_key_jitter_spread() {
        let policy = RuntimeCredentialCachePolicy::new(Duration::from_secs(60), Duration::from_secs(10)).unwrap();
        let cache = RuntimeCredentialCache::with_policy(policy);
        let credential_a = cache_credential("credential-jitter-a");
        let credential_b = cache_credential("credential-jitter-b");
        let now = Instant::now();

        cache.put_at(credential_a.clone(), cache_secret("token-a"), now).await;
        cache.put_at(credential_b.clone(), cache_secret("token-b"), now).await;

        let expires_a = cache.expires_at(&credential_a).await.unwrap();
        let expires_b = cache.expires_at(&credential_b).await.unwrap();

        assert_ne!(expires_a, expires_b);
    }

    fn timestamp(seconds: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(seconds, 0).unwrap()
    }

    #[test]
    fn revocation_latency_seconds_computes_elapsed_duration() {
        let recorded_at = timestamp(1_000);
        let now = timestamp(1_005);

        assert_eq!(revocation_latency_seconds(recorded_at, now), 5.0);
    }

    #[test]
    fn revocation_latency_seconds_clamps_negative_clock_skew_to_zero() {
        let recorded_at = timestamp(1_005);
        let now = timestamp(1_000);

        assert_eq!(revocation_latency_seconds(recorded_at, now), 0.0);
    }

    #[test]
    fn revocation_latency_seconds_is_zero_for_identical_timestamps() {
        let at = timestamp(1_000);

        assert_eq!(revocation_latency_seconds(at, at), 0.0);
    }

    #[tokio::test]
    async fn incremental_refresh_invalidates_cache_and_removes_projection_on_revoked_event() {
        let store = MockOpenBaoSecretStore::default();
        let credential = put_bot_token(&store, "Bot token").await;
        let metadata = store.metadata(&credential).await.unwrap();
        let event_store = ProjectionTestEventStore::default();
        event_store.push(credential.id().as_str(), 1, write_requested(&credential));
        event_store.push(
            credential.id().as_str(),
            2,
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        );
        let stream = raw_stream([
            runtime_event(1, write_requested(&credential)),
            runtime_event(
                2,
                v1::CredentialEvent {
                    event: Some(activated_to_proto(&metadata).into()),
                },
            ),
        ])
        .await;
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store.clone());

        registry
            .refresh_from_credential_stream_incremental(&stream, &event_store, 1)
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
        event_store.push(
            credential.id().as_str(),
            3,
            v1::CredentialEvent {
                event: Some(revoked_to_proto(&credential).into()),
            },
        );
        append_stream(
            &stream,
            StreamSubject::new("gateway.credentials.events.v1.raw").unwrap(),
            None,
            &[runtime_event(
                3,
                v1::CredentialEvent {
                    event: Some(revoked_to_proto(&credential).into()),
                },
            )],
        )
        .await
        .unwrap();

        let report = registry
            .refresh_from_credential_stream_incremental(&stream, &event_store, 3)
            .await
            .unwrap();

        assert_eq!(report.applied_credentials(), 1);
        assert!(matches!(
            resolver.resolve(&key(), CredentialKind::BotToken).await,
            Err(RuntimeCredentialError::IntegrationNotFound { .. })
        ));
    }
}
