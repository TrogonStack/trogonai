#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use trogon_decider_runtime::{
    EventDecodeOutcome, InvalidStreamPositionError, ReadFrom, ReadStreamRequest, StreamEvent, StreamPosition,
    StreamRead,
};
use trogon_nats::jetstream::{JetStreamGetRawMessage, JetStreamGetStreamInfo};
use trogon_semconv::{attribute, metric};
use trogon_std::SecretString;
use trogonai_proto::gateway::credentials::{
    CredentialEventCase, CredentialEventPayloadError, CredentialStateSnapshotCase, state_v1, v1,
};

use crate::constants::METER_NAME;
use crate::credential::commands::domain::{
    CredentialId, CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, RuntimeReleaseDeniedError,
    RuntimeReleasePolicy, RuntimeReleaseRequest, SourceKind,
};
use crate::credential::commands::proto::{
    CredentialProtoDecodeError, active_credential_ref, decode_cleanup_failed_state, decode_credential_id,
    decode_credential_metadata, decode_destroy_failed, decode_destroy_requested, decode_destroy_requested_state,
    decode_destroyed, decode_destroyed_state, decode_message_field, decode_revoked, decode_revoked_state,
    decode_rotated, decode_rotation_failed, decode_rotation_requested, decode_scope_key, decode_write_failed,
    decode_write_requested,
};
use crate::credential::constants::CREDENTIAL_EVENT_SUBJECT_PREFIX;
use crate::credential::event_stream::{CredentialEventStreamReadError, read_credential_event_stream};
use crate::credential::stream::{CredentialKey, CredentialSubject};
use crate::credential::{CredentialEvolveError, evolve, initial_state};
use crate::secret_store::{SecretMaterial, SecretStoreError, SecretStoreGet};
use crate::source_integration_id::SourceIntegrationId;

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
        let meter = trogon_telemetry::meter(METER_NAME);
        Self {
            revocation_latency: metric::build_gateway_credential_revocation_latency(&meter),
            cache_hits: metric::build_gateway_credential_cache_hits(&meter),
            cache_misses: metric::build_gateway_credential_cache_misses(&meter),
            delivery_denials: metric::build_gateway_credential_delivery_denied(&meter),
            resolve_failures: metric::build_gateway_credential_resolve_failures(&meter),
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
    fn record_delivery_denial(&self, key: &RuntimeIntegrationKey, denied: &RuntimeReleaseDeniedError) {
        let mut attributes = resolution_attributes(key);
        attributes.push(KeyValue::new(attribute::REASON, denial_reason(denied)));
        self.delivery_denials.add(1, &attributes);
    }

    fn record_resolve_failure(&self, key: &RuntimeIntegrationKey) {
        self.resolve_failures.add(1, &resolution_attributes(key));
    }
}

fn resolution_attributes(key: &RuntimeIntegrationKey) -> Vec<KeyValue> {
    vec![KeyValue::new(attribute::SOURCE, key.source().as_str().to_string())]
}

fn denial_reason(denied: &RuntimeReleaseDeniedError) -> &'static str {
    match denied {
        RuntimeReleaseDeniedError::RuntimeService { .. } => "runtime_service",
        RuntimeReleaseDeniedError::Host { .. } => "host",
        RuntimeReleaseDeniedError::InjectionLocation { .. } => "injection_location",
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
        let scope = decode_scope_key(
            credential.owner_id().clone(),
            credential.source(),
            "scope_key",
            credential.scope_key(),
        )
        .map_err(RuntimeProjectionBuildError::InvalidState)?;

        Ok(match scope {
            CredentialScope::Source { source, .. } => Self::for_source(source),
            CredentialScope::Integration {
                source, integration_id, ..
            } => Self::new(source, &integration_id),
        })
    }
}

/// `owner_id` is the project id: ADR#0046 collapses workspace-shaped fields
/// into the project, so the release policy's `workspace_id` is this field rather
/// than a second one beside it.
#[derive(Clone, Debug)]
pub struct RuntimeIntegrationProjection {
    key: RuntimeIntegrationKey,
    owner_id: CredentialOwnerId,
    status: RuntimeIntegrationStatus,
    version: u64,
    credentials: BTreeMap<CredentialKind, CredentialRef>,
    release_policy: RuntimeReleasePolicy,
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
            release_policy: RuntimeReleasePolicy::default(),
        }
    }

    pub fn with_credential(mut self, kind: CredentialKind, credential: CredentialRef) -> Self {
        self.credentials.insert(kind, credential);
        self
    }

    pub fn with_release_policy(mut self, release_policy: RuntimeReleasePolicy) -> Self {
        self.release_policy = release_policy;
        self
    }

    fn insert_credential(&mut self, credential: CredentialRef) -> Option<CredentialRef> {
        self.credentials.insert(credential.kind(), credential)
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
            release_policy: RuntimeReleasePolicy::default(),
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

    pub fn release_policy(&self) -> &RuntimeReleasePolicy {
        &self.release_policy
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
        release_policy: RuntimeReleasePolicy::default(),
    }))
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeProjectionBuildError {
    #[error("persisted credential state is invalid: {0}")]
    InvalidState(#[source] CredentialProtoDecodeError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeProjectionRefreshReport {
    scanned_events: usize,
    decoded_events: usize,
    skipped_events: usize,
    poisoned_events: usize,
    poisoned_credentials: usize,
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

    pub fn poisoned_events(self) -> usize {
        self.poisoned_events
    }

    pub fn poisoned_credentials(self) -> usize {
        self.poisoned_credentials
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
            || self.poisoned_events > 0
            || self.poisoned_credentials > 0
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
    #[error(
        "credential event at stream position {stream_position} on '{stream_id}' cannot be attributed to a credential"
    )]
    UnattributableEvent { stream_id: String, stream_position: u64 },
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

pub async fn run_tracked_refresh_worker<EventStream, EventStore>(
    registry: RuntimeCredentialRegistry,
    event_stream: EventStream,
    event_store: EventStore,
    interval: Duration,
    shutdown: impl Future<Output = ()>,
) where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
{
    let mut interval = tokio::time::interval(interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                info!("credential runtime projection refresh worker stopped");
                return;
            }
            _ = interval.tick() => {
                match registry
                    .refresh_from_credential_stream_tracked(&event_stream, &event_store)
                    .await
                {
                    Ok(report) if report.has_projection_activity() => {
                        let scanned_events = report.scanned_events();
                        let decoded_events = report.decoded_events();
                        let skipped_events = report.skipped_events();
                        let poisoned_events = report.poisoned_events();
                        let poisoned_credentials = report.poisoned_credentials();
                        let changed_credentials = report.changed_credentials();
                        let applied_credentials = report.applied_credentials();
                        let projected_integrations = report.projected_integrations();
                        let checkpoint_loaded_sequence = report.checkpoint_loaded_sequence();
                        let checkpoint_advanced_to = report.checkpoint_advanced_to();
                        info!(
                            scanned_events,
                            decoded_events,
                            skipped_events,
                            poisoned_events,
                            poisoned_credentials,
                            changed_credentials,
                            applied_credentials,
                            projected_integrations,
                            checkpoint_loaded_sequence,
                            checkpoint_advanced_to = ?checkpoint_advanced_to,
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

#[derive(Clone, Debug, Default)]
pub struct RuntimeProjectionCursor {
    last_scanned_sequence: Arc<AtomicU64>,
}

impl RuntimeProjectionCursor {
    fn checkpoint(&self) -> RuntimeProjectionCheckpoint {
        RuntimeProjectionCheckpoint::new(self.last_scanned_sequence.load(Ordering::Acquire))
    }

    fn advance(&self, last_scanned_sequence: u64) {
        self.last_scanned_sequence
            .fetch_max(last_scanned_sequence, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct RuntimeProjectionTable {
    integrations: BTreeMap<RuntimeIntegrationKey, RuntimeIntegrationProjection>,
    applied_positions: BTreeMap<CredentialId, StreamPosition>,
}

impl RuntimeProjectionTable {
    fn is_stale(&self, change: &RuntimeProjectionChange, position: StreamPosition) -> bool {
        change.credential_ids().any(|id| {
            self.applied_positions
                .get(id)
                .is_some_and(|applied| *applied >= position)
        })
    }

    fn record(&mut self, change: &RuntimeProjectionChange, position: StreamPosition) {
        for id in change.credential_ids() {
            self.applied_positions.insert(id.clone(), position);
        }
    }

    fn remove_credential(&mut self, key: &RuntimeIntegrationKey, kind: CredentialKind) -> Option<CredentialRef> {
        let projection = self.integrations.get_mut(key)?;
        let removed = projection.remove_credential(kind);
        if projection.is_empty() {
            self.integrations.remove(key);
        }
        removed
    }

    fn remove_credential_id(&mut self, id: &CredentialId) -> Vec<CredentialRef> {
        let mut removed = Vec::new();
        for projection in self.integrations.values_mut() {
            let kinds = projection
                .credentials
                .iter()
                .filter(|(_, credential)| credential.id() == id)
                .map(|(kind, _)| *kind)
                .collect::<Vec<_>>();
            for kind in kinds {
                removed.extend(projection.remove_credential(kind));
            }
        }
        self.integrations.retain(|_, projection| !projection.is_empty());
        removed
    }
}

enum RuntimeProjectionChange {
    Publish(RuntimeIntegrationProjection),
    Retire {
        key: RuntimeIntegrationKey,
        credential: CredentialRef,
    },
    Unpublish(CredentialId),
}

impl RuntimeProjectionChange {
    fn from_state(
        state: &state_v1::CredentialStateSnapshot,
        stream_position: StreamPosition,
    ) -> Result<Option<Self>, RuntimeProjectionRefreshError> {
        if let Some(projection) = RuntimeIntegrationProjection::from_credential_state(state, stream_position.as_u64())
            .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?
        {
            return Ok(Some(Self::Publish(projection)));
        }
        if let Some(credential) = post_revocation_credential_ref(state)
            .map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })?
        {
            let key = RuntimeIntegrationKey::from_credential_ref(&credential)
                .map_err(|source| RuntimeProjectionRefreshError::BuildProjection { source })?;
            return Ok(Some(Self::Retire { key, credential }));
        }
        Ok(unpublished_credential_id(state)
            .map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })?
            .map(Self::Unpublish))
    }

    fn credential_ids(&self) -> Box<dyn Iterator<Item = &CredentialId> + '_> {
        match self {
            Self::Publish(projection) => Box::new(projection.credentials.values().map(CredentialRef::id)),
            Self::Retire { credential, .. } => Box::new(std::iter::once(credential.id())),
            Self::Unpublish(id) => Box::new(std::iter::once(id)),
        }
    }
}

#[derive(Clone, Default)]
pub struct InMemoryRuntimeProjectionRepository {
    table: Arc<Mutex<RuntimeProjectionTable>>,
}

impl InMemoryRuntimeProjectionRepository {
    pub async fn upsert(&self, projection: RuntimeIntegrationProjection) {
        self.table
            .lock()
            .await
            .integrations
            .insert(projection.key().clone(), projection);
    }

    pub async fn replace_all(&self, projections: BTreeMap<RuntimeIntegrationKey, RuntimeIntegrationProjection>) {
        let mut table = self.table.lock().await;
        table.applied_positions.clear();
        for projection in projections.values() {
            if let Ok(position) = StreamPosition::try_new(projection.version()) {
                for credential in projection.credentials.values() {
                    table.applied_positions.insert(credential.id().clone(), position);
                }
            }
        }
        table.integrations = projections;
    }

    pub async fn len(&self) -> usize {
        self.table.lock().await.integrations.len()
    }

    pub async fn remove(&self, key: &RuntimeIntegrationKey) {
        self.table.lock().await.integrations.remove(key);
    }

    pub async fn merge(
        &self,
        projection: RuntimeIntegrationProjection,
    ) -> Result<Vec<CredentialRef>, RuntimeProjectionRefreshError> {
        merge_projection(&mut self.table.lock().await.integrations, projection)
    }

    pub async fn remove_credential(&self, key: &RuntimeIntegrationKey, kind: CredentialKind) -> Option<CredentialRef> {
        self.table.lock().await.remove_credential(key, kind)
    }

    async fn apply_change(
        &self,
        change: RuntimeProjectionChange,
        position: StreamPosition,
    ) -> Result<Option<Vec<CredentialRef>>, RuntimeProjectionRefreshError> {
        let mut table = self.table.lock().await;
        if table.is_stale(&change, position) {
            return Ok(None);
        }
        let affected = match &change {
            RuntimeProjectionChange::Publish(projection) => {
                let mut affected = projection.credentials.values().cloned().collect::<Vec<_>>();
                affected.extend(merge_projection(&mut table.integrations, projection.clone())?);
                affected
            }
            RuntimeProjectionChange::Retire { key, credential } => {
                let mut affected = vec![credential.clone()];
                affected.extend(table.remove_credential(key, credential.kind()));
                affected
            }
            RuntimeProjectionChange::Unpublish(id) => table.remove_credential_id(id),
        };
        table.record(&change, position);
        Ok(Some(affected))
    }

    async fn credential_ids(&self) -> BTreeSet<CredentialId> {
        self.table
            .lock()
            .await
            .integrations
            .values()
            .flat_map(|projection| {
                projection
                    .credentials
                    .values()
                    .map(|credential| credential.id().clone())
            })
            .collect()
    }

    async fn remove_credential_id(&self, id: &CredentialId) -> Vec<CredentialRef> {
        self.table.lock().await.remove_credential_id(id)
    }

    async fn get(&self, key: &RuntimeIntegrationKey) -> Option<RuntimeIntegrationProjection> {
        self.table.lock().await.integrations.get(key).cloned()
    }
}

#[derive(Clone, Default)]
pub struct RuntimeCredentialRegistry {
    projections: InMemoryRuntimeProjectionRepository,
    cache: RuntimeCredentialCache,
    cursor: RuntimeProjectionCursor,
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

    pub async fn refresh_from_credential_stream_tracked<EventStream, EventStore>(
        &self,
        event_stream: &EventStream,
        event_store: &EventStore,
    ) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionStreamRefreshError>
    where
        EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
        EventStore: StreamRead<str>,
        <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
    {
        refresh_runtime_projections_from_credential_stream_tracked(
            &self.projections,
            &self.cache,
            event_stream,
            event_store,
            &self.cursor,
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
        apply_state_to_projection(&self.projections, &self.cache, state, stream_position, None).await
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

pub async fn refresh_runtime_projections_from_credential_stream_tracked<EventStream, EventStore>(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    event_stream: &EventStream,
    event_store: &EventStore,
    cursor: &RuntimeProjectionCursor,
) -> Result<RuntimeProjectionRefreshReport, RuntimeProjectionStreamRefreshError>
where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>,
    <EventStore as StreamRead<str>>::Error: Error + Send + Sync + 'static,
{
    let checkpoint = cursor.checkpoint();
    let mut report = refresh_runtime_projections_from_credential_stream_incremental(
        projections,
        cache,
        event_stream,
        event_store,
        checkpoint.next_sequence(),
    )
    .await?;
    report.checkpoint_loaded_sequence = checkpoint.last_scanned_sequence();

    if let Some(last_scanned_sequence) = report.checkpoint_advanced_to() {
        cursor.advance(last_scanned_sequence);
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
        let version = stream_events.last().map_or(0, |(position, _)| *position);
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
    let mut poisoned_subjects = Vec::<(u64, String)>::new();

    for event in events {
        report.scanned_events += 1;
        let stream_position = event.stream_position.as_u64();
        let recorded_at = event.recorded_at;
        report.checkpoint_advanced_to = Some(report.checkpoint_advanced_to.unwrap_or(0).max(stream_position));
        let decoded = match event.decode::<v1::CredentialEvent>() {
            Ok(EventDecodeOutcome::Decoded(decoded)) => decoded,
            Ok(EventDecodeOutcome::Skipped) => {
                report.skipped_events += 1;
                continue;
            }
            Err(source) => {
                warn!(stream_position, error = %source, "undecodable credential event");
                report.poisoned_events += 1;
                poisoned_subjects.push((stream_position, event.stream_id().to_string()));
                continue;
            }
        };
        report.decoded_events += 1;
        let credential_id = match decoded
            .event
            .as_ref()
            .ok_or(RuntimeProjectionRefreshError::MissingEvent)
            .and_then(|case| {
                event_credential_id(case)
                    .map(|id| (id, matches!(case, CredentialEventCase::Revoked(_))))
                    .map_err(|source| RuntimeProjectionRefreshError::InvalidEvent { source })
            }) {
            Ok((credential_id, revoked)) => {
                if revoked {
                    revoked_recorded_at.insert(credential_id.clone(), recorded_at);
                }
                credential_id
            }
            Err(source) => {
                warn!(stream_position, error = %source, "invalid credential event");
                report.poisoned_events += 1;
                poisoned_subjects.push((stream_position, event.stream_id().to_string()));
                continue;
            }
        };
        changed_credentials
            .entry(credential_id)
            .and_modify(|position| *position = (*position).max(stream_position))
            .or_insert(stream_position);
    }

    let mut known_subjects = BTreeMap::<String, CredentialId>::new();
    for credential_id in projections
        .credential_ids()
        .await
        .into_iter()
        .chain(changed_credentials.keys().cloned())
    {
        if let Ok(subject) = CredentialSubject::event(&CredentialKey::derive(&credential_id)) {
            known_subjects.insert(subject.as_str().to_string(), credential_id);
        }
    }
    let mut poisoned_credentials = BTreeSet::<CredentialId>::new();
    for (stream_position, stream_id) in poisoned_subjects {
        match known_subjects.get(&stream_id) {
            Some(credential_id) => {
                poisoned_credentials.insert(credential_id.clone());
            }
            None if is_credential_event_subject(&stream_id) => {
                warn!(stream_position, %stream_id, "poisoned event belongs to a credential that is not projected");
            }
            None => {
                return Err(RuntimeProjectionRefreshError::UnattributableEvent {
                    stream_id,
                    stream_position,
                });
            }
        }
    }
    for credential_id in &poisoned_credentials {
        changed_credentials.remove(credential_id);
        error!(%credential_id, "evicting credential with an unreadable event");
        for evicted in projections.remove_credential_id(credential_id).await {
            cache.invalidate(&evicted).await;
        }
        report.poisoned_credentials += 1;
    }

    report.changed_credentials = changed_credentials.len();

    let mut transient_failure = None;
    for (credential_id, version) in changed_credentials {
        let revoked_event_recorded_at = revoked_recorded_at.get(&credential_id).copied();
        let changed_position = position(version)?;
        let applied = match load_credential_state(event_store, &credential_id).await {
            Ok((state, loaded_position)) => {
                apply_state_to_projection(
                    projections,
                    cache,
                    &state,
                    loaded_position.map_or(changed_position, |loaded| loaded.max(changed_position)),
                    revoked_event_recorded_at,
                )
                .await
            }
            Err(source) => Err(source),
        };
        match applied {
            Ok(()) => report.applied_credentials += 1,
            Err(source @ RuntimeProjectionRefreshError::ReadCredential { .. }) => {
                warn!(%credential_id, error = %source, "credential read failed, will retry");
                transient_failure.get_or_insert(source);
            }
            Err(source) => {
                error!(%credential_id, error = %source, "evicting credential whose state cannot be projected");
                for evicted in projections.remove_credential_id(&credential_id).await {
                    cache.invalidate(&evicted).await;
                }
                report.poisoned_credentials += 1;
            }
        }
    }
    if let Some(source) = transient_failure {
        return Err(source);
    }
    report.projected_integrations = projections.len().await;

    Ok(report)
}

fn is_credential_event_subject(stream_id: &str) -> bool {
    stream_id
        .strip_prefix(CREDENTIAL_EVENT_SUBJECT_PREFIX)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|key| key.len() == 32 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

async fn load_credential_state<EventStore>(
    event_store: &EventStore,
    credential_id: &CredentialId,
) -> Result<(state_v1::CredentialStateSnapshot, Option<StreamPosition>), RuntimeProjectionRefreshError>
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
    let mut last_position = stream.current_position;
    for event in stream.events {
        last_position = last_position.max(Some(event.stream_position));
        let EventDecodeOutcome::Decoded(event) = event
            .decode::<v1::CredentialEvent>()
            .map_err(|source| RuntimeProjectionRefreshError::DecodeEvent { source })?
        else {
            continue;
        };
        state = evolve(state, &event).map_err(|source| RuntimeProjectionRefreshError::ReplayStream { source })?;
    }
    Ok((state, last_position))
}

async fn invalidate_all(cache: &RuntimeCredentialCache, credentials: &[CredentialRef]) {
    for credential in credentials {
        cache.invalidate(credential).await;
    }
}

async fn apply_state_to_projection(
    projections: &InMemoryRuntimeProjectionRepository,
    cache: &RuntimeCredentialCache,
    state: &state_v1::CredentialStateSnapshot,
    stream_position: StreamPosition,
    revoked_event_recorded_at: Option<DateTime<Utc>>,
) -> Result<(), RuntimeProjectionRefreshError> {
    let Some(change) = RuntimeProjectionChange::from_state(state, stream_position)? else {
        return Ok(());
    };
    let Some(affected) = projections.apply_change(change, stream_position).await? else {
        return Ok(());
    };
    invalidate_all(cache, &affected).await;
    if matches!(state.state, Some(CredentialStateSnapshotCase::Revoked(_)))
        && let Some(recorded_at) = revoked_event_recorded_at
    {
        runtime_projection_metrics().record_revocation_latency(recorded_at);
    }
    Ok(())
}

fn unpublished_credential_id(
    state: &state_v1::CredentialStateSnapshot,
) -> Result<Option<CredentialId>, CredentialProtoDecodeError> {
    match state.state.as_ref() {
        Some(CredentialStateSnapshotCase::PendingWrite(pending)) => Ok(Some(decode_credential_id(
            "pending_write.credential_id",
            &pending.credential_id,
        )?)),
        Some(CredentialStateSnapshotCase::WriteFailed(failed)) => Ok(Some(decode_credential_id(
            "write_failed.credential_id",
            &failed.credential_id,
        )?)),
        None
        | Some(
            CredentialStateSnapshotCase::Missing(_)
            | CredentialStateSnapshotCase::Active(_)
            | CredentialStateSnapshotCase::RotationPending(_)
            | CredentialStateSnapshotCase::Revoked(_)
            | CredentialStateSnapshotCase::DestroyRequested(_)
            | CredentialStateSnapshotCase::Destroyed(_)
            | CredentialStateSnapshotCase::CleanupFailed(_),
        ) => Ok(None),
    }
}

fn post_revocation_credential_ref(
    state: &state_v1::CredentialStateSnapshot,
) -> Result<Option<CredentialRef>, CredentialProtoDecodeError> {
    match state.state.as_ref() {
        Some(CredentialStateSnapshotCase::Revoked(revoked)) => Ok(Some(decode_revoked_state(revoked)?)),
        Some(CredentialStateSnapshotCase::DestroyRequested(pending)) => {
            let (credential_ref, _reason) = decode_destroy_requested_state(pending)?;
            Ok(Some(credential_ref))
        }
        Some(CredentialStateSnapshotCase::Destroyed(destroyed)) => Ok(Some(decode_destroyed_state(destroyed)?)),
        Some(CredentialStateSnapshotCase::CleanupFailed(cleanup_failed)) => {
            let (credential_ref, _reason) = decode_cleanup_failed_state(cleanup_failed)?;
            Ok(Some(credential_ref))
        }
        None
        | Some(
            CredentialStateSnapshotCase::Missing(_)
            | CredentialStateSnapshotCase::PendingWrite(_)
            | CredentialStateSnapshotCase::WriteFailed(_)
            | CredentialStateSnapshotCase::Active(_)
            | CredentialStateSnapshotCase::RotationPending(_),
        ) => Ok(None),
    }
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
) -> Result<Vec<CredentialRef>, RuntimeProjectionRefreshError> {
    let mut replaced = Vec::new();
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
                replaced.extend(existing.insert_credential(credential));
            }
            existing.advance_version(version);
        }
        None => {
            projections.insert(key, projection);
        }
    }
    Ok(replaced)
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
    /// `RuntimeReleasePolicy` cache override.
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

    pub async fn active_credential_ref(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
    ) -> Option<CredentialRef> {
        let projection = self.projections.get(key).await?;
        if !projection.status().is_resolvable() {
            return None;
        }
        projection.credential(kind).cloned()
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
        self.resolve_for(key, kind, &RuntimeReleaseRequest::new()).await
    }

    /// Resolves under an explicit release request so the projection's policy
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
        request: &RuntimeReleaseRequest<'_>,
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

        projection.release_policy().permits(request).map_err(|denied| {
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
            .release_policy()
            .effective_cache_ttl(self.cache.policy().ttl());
        self.cache.put_with_ttl(credential.clone(), material.clone(), ttl).await;
        Ok(material)
    }

    /// Reads one exact credential version straight from the store.
    ///
    /// The cache is skipped on purpose: a version that is no longer the
    /// projected one is not covered by revocation invalidation, so only the
    /// store's own status check can fail it closed.
    pub async fn read_uncached(&self, credential: &CredentialRef) -> Result<SecretMaterial, SecretStoreError> {
        self.store.get(credential).await
    }

    pub async fn resolve_plaintext(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
    ) -> Result<SecretString, RuntimeCredentialError> {
        self.resolve_plaintext_for(key, kind, &RuntimeReleaseRequest::new())
            .await
    }

    pub async fn resolve_plaintext_for(
        &self,
        key: &RuntimeIntegrationKey,
        kind: CredentialKind,
        request: &RuntimeReleaseRequest<'_>,
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
        denied: RuntimeReleaseDeniedError,
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

    pub fn http_status(&self) -> axum::http::StatusCode {
        match self {
            Self::SecretStore(SecretStoreError::BackendUnavailable { .. }) => {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            }
            Self::SecretStore(_)
            | Self::IntegrationNotFound { .. }
            | Self::IntegrationNotResolvable { .. }
            | Self::CredentialMissing { .. }
            | Self::VerifierOnly { .. }
            | Self::DeliveryDenied { .. } => axum::http::StatusCode::UNAUTHORIZED,
        }
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
mod tests;
