//! Snapshot storage helpers backed by JetStream Key/Value.
//!
//! Snapshots are stored under NATS-specific keys derived from the
//! [`SnapshotType::snapshot_type`] identity and the caller's snapshot id.
//! Checkpoints are optional and use a `snapshots.checkpoint.*` key so snapshot
//! listings can separate stream snapshots from adapter metadata.

use async_nats::jetstream::kv;
use bytes::Bytes;
use futures::StreamExt;
use opentelemetry::global;
use opentelemetry::metrics::Counter;
use std::sync::OnceLock;
use std::{borrow::Cow, collections::BTreeMap, convert::Infallible};
use trogon_decider_runtime::StreamPosition;
use trogon_decider_runtime::snapshot::{
    EncodedSnapshot, Snapshot, SnapshotEnvelopeDecodeError, SnapshotEnvelopeEncodeError, SnapshotPayloadDecode,
    SnapshotPayloadEncode, SnapshotType, SnapshotTypeName, decode_snapshot, encode_snapshot,
};
use trogon_nats::jetstream::{
    JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry,
    JetStreamKvGet, JetStreamKvKeys,
};
use trogon_semconv::metric;

const METER_NAME: &str = "trogon-decider-nats";

struct SnapshotStoreMetrics {
    kv_read_failures: Counter<u64>,
    kv_write_failures: Counter<u64>,
}

impl SnapshotStoreMetrics {
    fn new() -> Self {
        let meter = global::meter(METER_NAME);
        Self {
            kv_read_failures: metric::build_decider_snapshot_kv_read_failures(&meter),
            kv_write_failures: metric::build_decider_snapshot_kv_write_failures(&meter),
        }
    }
}

static METRICS: OnceLock<SnapshotStoreMetrics> = OnceLock::new();

fn metrics() -> &'static SnapshotStoreMetrics {
    METRICS.get_or_init(SnapshotStoreMetrics::new)
}

fn record_kv_failure(error: &SnapshotKvError) {
    match error {
        SnapshotKvError::ListSnapshotKeys { .. }
        | SnapshotKvError::ReadSnapshotKey { .. }
        | SnapshotKvError::ReadSnapshotValue { .. }
        | SnapshotKvError::ReadSnapshotEntry { .. }
        | SnapshotKvError::ReadCheckpointEntry { .. }
        | SnapshotKvError::ReadEntryForUpdate { .. }
        | SnapshotKvError::ReadEntryForSnapshotUpdate { .. }
        | SnapshotKvError::ReadEntryForDelete { .. } => {
            metrics().kv_read_failures.add(1, &[]);
        }
        SnapshotKvError::AdvanceCheckpoint { .. }
        | SnapshotKvError::CreateCheckpoint { .. }
        | SnapshotKvError::UpdateEntry { .. }
        | SnapshotKvError::CreateEntry { .. }
        | SnapshotKvError::UpdateSnapshotEntry { .. }
        | SnapshotKvError::CreateSnapshotEntry { .. }
        | SnapshotKvError::DeleteEntry { .. } => {
            metrics().kv_write_failures.add(1, &[]);
        }
        SnapshotKvError::DecodeCheckpoint { .. } => {}
    }
}

/// Bound covering all KV bucket operations used by snapshot helpers.
pub trait SnapshotKvBucket:
    JetStreamKvGet
    + JetStreamKvEntry
    + JetStreamKvCreate
    + JetStreamKvKeys
    + JetStreamKeyValueUpdate
    + JetStreamKeyValueDeleteExpectRevision
{
}

impl<T> SnapshotKvBucket for T where
    T: JetStreamKvGet
        + JetStreamKvEntry
        + JetStreamKvCreate
        + JetStreamKvKeys
        + JetStreamKeyValueUpdate
        + JetStreamKeyValueDeleteExpectRevision
{
}

#[derive(Debug, thiserror::Error)]
/// JetStream Key/Value error raised by snapshot storage.
#[allow(missing_docs)]
pub enum SnapshotKvError {
    #[error("failed to list stream snapshot keys: {source}")]
    ListSnapshotKeys {
        #[source]
        source: kv::HistoryError,
    },
    #[error("failed to read stream snapshot key: {source}")]
    ReadSnapshotKey {
        #[source]
        source: kv::WatcherError,
    },
    #[error("failed to read stream snapshot value: {source}")]
    ReadSnapshotValue {
        #[source]
        source: kv::EntryError,
    },
    #[error("failed to read stream snapshot entry: {source}")]
    ReadSnapshotEntry {
        #[source]
        source: kv::EntryError,
    },
    #[error("failed to advance stream snapshot checkpoint: {source}")]
    AdvanceCheckpoint {
        #[source]
        source: kv::UpdateError,
    },
    #[error("failed to create stream snapshot checkpoint: {source}")]
    CreateCheckpoint {
        #[source]
        source: kv::CreateError,
    },
    #[error("failed to read stream snapshot checkpoint entry: {source}")]
    ReadCheckpointEntry {
        #[source]
        source: kv::EntryError,
    },
    #[error("failed to decode stream snapshot checkpoint: {key}")]
    DecodeCheckpoint { key: String },
    #[error("failed to read key-value entry for update: {source}")]
    ReadEntryForUpdate {
        #[source]
        source: kv::EntryError,
    },
    #[error("failed to update key-value entry: {source}")]
    UpdateEntry {
        #[source]
        source: kv::UpdateError,
    },
    #[error("failed to create key-value entry: {source}")]
    CreateEntry {
        #[source]
        source: kv::CreateError,
    },
    #[error("failed to read key-value entry for snapshot update: {source}")]
    ReadEntryForSnapshotUpdate {
        #[source]
        source: kv::EntryError,
    },
    #[error("failed to update snapshot entry: {source}")]
    UpdateSnapshotEntry {
        #[source]
        source: kv::UpdateError,
    },
    #[error("failed to create snapshot entry: {source}")]
    CreateSnapshotEntry {
        #[source]
        source: kv::CreateError,
    },
    #[error("failed to read key-value entry for delete: {source}")]
    ReadEntryForDelete {
        #[source]
        source: kv::EntryError,
    },
    #[error("failed to delete key-value entry: {source}")]
    DeleteEntry {
        #[source]
        source: kv::DeleteError,
    },
}

#[derive(Debug, thiserror::Error)]
/// Snapshot envelope or payload codec error.
#[allow(missing_docs)]
pub enum SnapshotCodecError<PayloadError, SnapshotTypeError = Infallible> {
    #[error("failed to resolve stream snapshot type: {source}")]
    SnapshotType {
        #[source]
        source: SnapshotTypeError,
    },
    #[error("failed to encode stream snapshot payload: {source}")]
    EncodePayload {
        #[source]
        source: PayloadError,
    },
    #[error("failed to encode stream snapshot envelope: {source}")]
    EncodeEnvelope {
        #[source]
        source: SnapshotEnvelopeEncodeError,
    },
    #[error("failed to decode stream snapshot envelope: {source}")]
    DecodeEnvelope {
        #[source]
        source: SnapshotEnvelopeDecodeError,
    },
    #[error("unexpected stream snapshot type: expected {expected}, got {actual}")]
    UnexpectedSnapshotType { expected: SnapshotTypeName, actual: String },
    #[error("failed to decode stream snapshot payload: {source}")]
    DecodePayload {
        #[source]
        source: PayloadError,
    },
}

#[derive(Debug, thiserror::Error)]
/// Error raised while reading, writing, or decoding snapshot state.
pub enum SnapshotStoreError<PayloadError = Infallible, SnapshotTypeError = Infallible> {
    /// JetStream Key/Value operation failed.
    #[error("KV error: {0}")]
    Kv(#[source] SnapshotKvError),
    /// Snapshot envelope or payload encoding failed.
    #[error("Snapshot codec error: {0}")]
    Codec(#[source] SnapshotCodecError<PayloadError, SnapshotTypeError>),
    /// A key used the snapshot namespace prefix but did not include a snapshot id.
    #[error("Invalid stream snapshot key: {key}")]
    InvalidSnapshotKey {
        /// Invalid key observed in the Key/Value bucket.
        key: String,
    },
    /// Checkpoint operations were requested without a checkpoint name.
    #[error("Missing checkpoint name for snapshot type: {snapshot_type}")]
    MissingCheckpointName {
        /// Snapshot type that requires a checkpoint name.
        snapshot_type: SnapshotTypeName,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Snapshot checkpoint configuration for the NATS adapter.
pub struct NatsSnapshotConfig {
    checkpoint_name: Option<Cow<'static, str>>,
}

impl NatsSnapshotConfig {
    /// Disables checkpoint reads and writes.
    pub const fn without_checkpoint() -> Self {
        Self { checkpoint_name: None }
    }

    /// Enables checkpoint reads and writes using the supplied checkpoint name.
    pub fn with_checkpoint_name(checkpoint_name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            checkpoint_name: Some(checkpoint_name.into()),
        }
    }

    /// Returns the configured checkpoint name when checkpointing is enabled.
    pub fn checkpoint_name(&self) -> Option<&str> {
        self.checkpoint_name.as_deref()
    }
}

impl Default for NatsSnapshotConfig {
    fn default() -> Self {
        Self::without_checkpoint()
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Persisted snapshot mutation.
pub enum SnapshotChange<T> {
    /// Writes a snapshot for the snapshot id.
    Upsert {
        /// Domain snapshot id associated with the snapshot.
        snapshot_id: String,
        /// Snapshot payload and stream position to store.
        snapshot: Box<Snapshot<T>>,
    },
    /// Deletes a snapshot for the snapshot id.
    Delete {
        /// Domain snapshot id whose snapshot should be removed.
        snapshot_id: String,
    },
}

impl<T> SnapshotChange<T> {
    /// Builds an upsert change for a stream snapshot.
    pub fn upsert(snapshot_id: impl Into<String>, snapshot: Snapshot<T>) -> Self {
        Self::Upsert {
            snapshot_id: snapshot_id.into(),
            snapshot: Box::new(snapshot),
        }
    }

    /// Builds a delete change for a stream snapshot.
    pub fn delete(snapshot_id: impl Into<String>) -> Self {
        Self::Delete {
            snapshot_id: snapshot_id.into(),
        }
    }
}

impl<PayloadError, SnapshotTypeError> From<SnapshotKvError> for SnapshotStoreError<PayloadError, SnapshotTypeError> {
    fn from(source: SnapshotKvError) -> Self {
        record_kv_failure(&source);
        Self::Kv(source)
    }
}

impl<PayloadError, SnapshotTypeError> From<SnapshotCodecError<PayloadError, SnapshotTypeError>>
    for SnapshotStoreError<PayloadError, SnapshotTypeError>
{
    fn from(source: SnapshotCodecError<PayloadError, SnapshotTypeError>) -> Self {
        Self::Codec(source)
    }
}

const SNAPSHOT_DATA_KEY_NAMESPACE: &str = "snapshots.data";
const SNAPSHOT_CHECKPOINT_KEY_NAMESPACE: &str = "snapshots.checkpoint";

type SnapshotTypeError<T> = <T as SnapshotType>::Error;
type SnapshotEncodePayloadError<T> = <T as SnapshotPayloadEncode>::Error;
type SnapshotDecodePayloadError<T> = <T as SnapshotPayloadDecode>::Error;

fn resolve_snapshot_type<T, PayloadError>()
-> Result<SnapshotTypeName, SnapshotStoreError<PayloadError, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    T::snapshot_type()
        .map_err(|source| SnapshotCodecError::SnapshotType { source })
        .map_err(Into::into)
}

fn snapshot_key_prefix<T, PayloadError>() -> Result<String, SnapshotStoreError<PayloadError, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    let snapshot_type = resolve_snapshot_type::<T, PayloadError>()?;
    Ok(format!("{}.{}.", SNAPSHOT_DATA_KEY_NAMESPACE, snapshot_type))
}

/// Builds the Key/Value key used to store a stream snapshot.
pub fn snapshot_key<T>(snapshot_id: &str) -> Result<String, SnapshotStoreError<Infallible, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    snapshot_key_for::<T, Infallible>(snapshot_id)
}

fn snapshot_key_for<T, PayloadError>(
    snapshot_id: &str,
) -> Result<String, SnapshotStoreError<PayloadError, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    Ok(format!("{}{}", snapshot_key_prefix::<T, PayloadError>()?, snapshot_id))
}

/// Builds the Key/Value key used to store a snapshot checkpoint.
pub fn checkpoint_key<T>(
    config: &NatsSnapshotConfig,
) -> Result<String, SnapshotStoreError<Infallible, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    let snapshot_type = resolve_snapshot_type::<T, Infallible>()?;

    let checkpoint_name = config
        .checkpoint_name()
        .ok_or_else(|| SnapshotStoreError::MissingCheckpointName {
            snapshot_type: snapshot_type.clone(),
        })?;

    Ok(format!(
        "{}.{}.{}",
        SNAPSHOT_CHECKPOINT_KEY_NAMESPACE, snapshot_type, checkpoint_name,
    ))
}

fn snapshot_id_from_snapshot_key<T, PayloadError>(
    key: &str,
) -> Result<Option<String>, SnapshotStoreError<PayloadError, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    let key_prefix = snapshot_key_prefix::<T, PayloadError>()?;
    let Some(snapshot_id) = key.strip_prefix(&key_prefix) else {
        return Ok(None);
    };

    if snapshot_id.is_empty() {
        return Err(SnapshotStoreError::InvalidSnapshotKey { key: key.to_string() });
    }

    Ok(Some(snapshot_id.to_string()))
}

fn snapshot_value<T>(
    snapshot: &Snapshot<T>,
) -> Result<Vec<u8>, SnapshotStoreError<SnapshotEncodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadEncode + SnapshotType,
    SnapshotEncodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    let encoded = encode_snapshot(snapshot).map_err(|source| match source {
        trogon_decider_runtime::snapshot::SnapshotEncodeError::SnapshotType { source } => {
            SnapshotCodecError::SnapshotType { source }
        }
        trogon_decider_runtime::snapshot::SnapshotEncodeError::Payload { source } => {
            SnapshotCodecError::EncodePayload { source }
        }
    })?;

    encoded
        .into_bytes()
        .map_err(|source| SnapshotCodecError::EncodeEnvelope { source }.into())
}

fn snapshot_from_value<T>(
    value: &[u8],
) -> Result<Snapshot<T>, SnapshotStoreError<SnapshotDecodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    SnapshotDecodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    let encoded = EncodedSnapshot::from_bytes(value).map_err(|source| SnapshotCodecError::DecodeEnvelope { source })?;
    decode_snapshot::<T>(encoded)
        .map_err(|source| match source {
            trogon_decider_runtime::snapshot::SnapshotDecodeError::SnapshotType { source } => {
                SnapshotCodecError::SnapshotType { source }
            }
            trogon_decider_runtime::snapshot::SnapshotDecodeError::Payload { source } => {
                SnapshotCodecError::DecodePayload { source }
            }
            trogon_decider_runtime::snapshot::SnapshotDecodeError::UnexpectedType { expected, actual } => {
                SnapshotCodecError::UnexpectedSnapshotType { expected, actual }
            }
        })
        .map_err(Into::into)
}

fn snapshot_position_from_value<T, PayloadError>(
    value: &[u8],
) -> Result<StreamPosition, SnapshotStoreError<PayloadError, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
{
    let encoded = EncodedSnapshot::from_bytes(value).map_err(|source| SnapshotCodecError::DecodeEnvelope { source })?;
    let snapshot_type = resolve_snapshot_type::<T, PayloadError>()?;
    if encoded.r#type != snapshot_type.as_str() {
        return Err(SnapshotCodecError::UnexpectedSnapshotType {
            expected: snapshot_type,
            actual: encoded.r#type,
        }
        .into());
    }

    Ok(encoded.position)
}

async fn read_snapshot_entries<T, K>(
    bucket: &K,
) -> Result<Vec<(String, Snapshot<T>)>, SnapshotStoreError<SnapshotDecodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    SnapshotDecodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet + JetStreamKvKeys,
{
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| SnapshotKvError::ListSnapshotKeys { source })?;
    let mut snapshots = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| SnapshotKvError::ReadSnapshotKey { source })?;
        let Some(snapshot_id) = snapshot_id_from_snapshot_key::<T, SnapshotDecodePayloadError<T>>(&key)? else {
            continue;
        };
        let Some(value) = bucket
            .get(key)
            .await
            .map_err(|source| SnapshotKvError::ReadSnapshotValue { source })?
        else {
            continue;
        };
        let snapshot = match snapshot_from_value::<T>(&value) {
            Ok(snapshot) => snapshot,
            Err(SnapshotStoreError::Codec(SnapshotCodecError::UnexpectedSnapshotType { .. })) => continue,
            Err(error) => return Err(error),
        };
        snapshots.push((snapshot_id, snapshot));
    }

    Ok(snapshots)
}

/// Reads the snapshot currently stored for the snapshot id.
pub async fn read_snapshot<T, K>(
    bucket: &K,
    snapshot_id: &str,
) -> Result<Option<Snapshot<T>>, SnapshotStoreError<SnapshotDecodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    SnapshotDecodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet,
{
    let Some(value) = bucket
        .get(snapshot_key_for::<T, SnapshotDecodePayloadError<T>>(snapshot_id)?)
        .await
        .map_err(|source| SnapshotKvError::ReadSnapshotEntry { source })?
    else {
        return Ok(None);
    };

    snapshot_from_value::<T>(&value).map(Some)
}

/// Writes a snapshot for the snapshot id.
pub async fn write_snapshot<T, K>(
    bucket: &K,
    snapshot_id: &str,
    snapshot: Snapshot<T>,
) -> Result<(), SnapshotStoreError<SnapshotEncodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadEncode + SnapshotType,
    SnapshotEncodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: SnapshotKvBucket,
{
    persist_snapshot_change(bucket, SnapshotChange::upsert(snapshot_id, snapshot)).await
}

/// Lists snapshots in the payload type namespace.
pub async fn list_snapshots<T, K>(
    bucket: &K,
) -> Result<Vec<Snapshot<T>>, SnapshotStoreError<SnapshotDecodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    SnapshotDecodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet + JetStreamKvKeys,
{
    read_snapshot_entries(bucket)
        .await
        .map(|entries| entries.into_iter().map(|(_, snapshot)| snapshot).collect())
}

/// Reads snapshots in the payload type namespace keyed by snapshot id.
pub async fn read_snapshot_map<T, K>(
    bucket: &K,
) -> Result<BTreeMap<String, Snapshot<T>>, SnapshotStoreError<SnapshotDecodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    SnapshotDecodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet + JetStreamKvKeys,
{
    read_snapshot_entries(bucket)
        .await
        .map(|entries| entries.into_iter().collect())
}

/// Reads the configured checkpoint sequence.
///
/// Missing or deleted checkpoint entries are treated as sequence `0`.
pub async fn read_checkpoint<T, K>(
    bucket: &K,
    config: &NatsSnapshotConfig,
) -> Result<u64, SnapshotStoreError<Infallible, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry,
{
    let (_revision, sequence) = read_checkpoint_entry::<T, K>(bucket, config).await?;
    Ok(sequence)
}

/// Writes the configured checkpoint sequence unconditionally.
pub async fn write_checkpoint<T, K>(
    bucket: &K,
    config: &NatsSnapshotConfig,
    sequence: u64,
) -> Result<(), SnapshotStoreError<Infallible, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    write_kv_value::<Infallible, SnapshotTypeError<T>, K>(
        bucket,
        &checkpoint_key::<T>(config)?,
        checkpoint_value(sequence),
    )
    .await
}

/// Advances the checkpoint only when the current value is exactly one behind.
///
/// This keeps concurrent snapshot workers from skipping gaps or rewinding the
/// checkpoint while still allowing the winning writer to move it forward.
pub async fn maybe_advance_checkpoint<T, K>(
    bucket: &K,
    config: &NatsSnapshotConfig,
    sequence: u64,
) -> Result<(), SnapshotStoreError<Infallible, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    let expected_previous = sequence.saturating_sub(1);
    let (revision, current_sequence) = read_checkpoint_entry::<T, K>(bucket, config).await?;
    if current_sequence != expected_previous {
        return Ok(());
    }

    let checkpoint_key = checkpoint_key::<T>(config)?;
    let value = checkpoint_value(sequence);
    match revision {
        Some(revision) => match bucket.update(&checkpoint_key, value.into(), revision).await {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(()),
            Err(source) => Err(SnapshotKvError::AdvanceCheckpoint { source }.into()),
        },
        None => match bucket.create(&checkpoint_key, value.into()).await {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => Ok(()),
            Err(source) => Err(SnapshotKvError::CreateCheckpoint { source }.into()),
        },
    }
}

/// Persists a snapshot mutation.
///
/// Upserts keep the highest snapshot position already present in the bucket;
/// deletes are idempotent.
pub async fn persist_snapshot_change<T, K>(
    bucket: &K,
    change: SnapshotChange<T>,
) -> Result<(), SnapshotStoreError<SnapshotEncodePayloadError<T>, SnapshotTypeError<T>>>
where
    T: SnapshotPayloadEncode + SnapshotType,
    SnapshotEncodePayloadError<T>: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: SnapshotKvBucket,
{
    match change {
        SnapshotChange::Upsert { snapshot_id, snapshot } => {
            let snapshot_position = snapshot.position;
            let value = snapshot_value(snapshot.as_ref())?;
            let snapshot_key = snapshot_key_for::<T, SnapshotEncodePayloadError<T>>(&snapshot_id)?;
            write_snapshot_value::<T, SnapshotEncodePayloadError<T>, K>(
                bucket,
                &snapshot_key,
                snapshot_position,
                value,
            )
            .await?;
        }
        SnapshotChange::Delete { snapshot_id } => {
            delete_kv_value::<SnapshotEncodePayloadError<T>, SnapshotTypeError<T>, K>(
                bucket,
                &snapshot_key_for::<T, SnapshotEncodePayloadError<T>>(&snapshot_id)?,
            )
            .await?;
        }
    }

    Ok(())
}

fn checkpoint_value(sequence: u64) -> Vec<u8> {
    sequence.to_string().into_bytes()
}

async fn read_checkpoint_entry<T, K>(
    bucket: &K,
    config: &NatsSnapshotConfig,
) -> Result<(Option<u64>, u64), SnapshotStoreError<Infallible, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry,
{
    let checkpoint_key = checkpoint_key::<T>(config)?;
    let Some(entry) = bucket
        .entry(checkpoint_key.clone())
        .await
        .map_err(|source| SnapshotKvError::ReadCheckpointEntry { source })?
    else {
        return Ok((None, 0));
    };
    if entry.operation != kv::Operation::Put {
        return Ok((None, 0));
    }

    let sequence = String::from_utf8(entry.value.to_vec())
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(SnapshotKvError::DecodeCheckpoint { key: checkpoint_key })?;

    Ok((Some(entry.revision), sequence))
}

async fn write_kv_value<PayloadError, SnapshotTypeError, K>(
    bucket: &K,
    key: &str,
    value: Vec<u8>,
) -> Result<(), SnapshotStoreError<PayloadError, SnapshotTypeError>>
where
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    let value = Bytes::from(value);
    loop {
        if let Some(entry) = bucket
            .entry(key.to_string())
            .await
            .map_err(|source| SnapshotKvError::ReadEntryForUpdate { source })?
        {
            match bucket.update(key, value.clone(), entry.revision).await {
                Ok(_) => return Ok(()),
                Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(source) => {
                    return Err(SnapshotKvError::UpdateEntry { source }.into());
                }
            }
        } else {
            match bucket.create(key, value.clone()).await {
                Ok(_) => return Ok(()),
                Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(SnapshotKvError::CreateEntry { source }.into());
                }
            }
        }
    }
}

async fn write_snapshot_value<T, PayloadError, K>(
    bucket: &K,
    key: &str,
    snapshot_position: StreamPosition,
    value: Vec<u8>,
) -> Result<(), SnapshotStoreError<PayloadError, SnapshotTypeError<T>>>
where
    T: SnapshotType,
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError<T>: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    loop {
        let Some(entry) = bucket
            .entry(key.to_string())
            .await
            .map_err(|source| SnapshotKvError::ReadEntryForSnapshotUpdate { source })?
        else {
            if create_snapshot_value::<PayloadError, SnapshotTypeError<T>, K>(bucket, key, value.clone()).await? {
                return Ok(());
            }
            continue;
        };

        if entry.operation != kv::Operation::Put {
            if create_snapshot_value::<PayloadError, SnapshotTypeError<T>, K>(bucket, key, value.clone()).await? {
                return Ok(());
            }
            continue;
        }

        if snapshot_position_from_value::<T, PayloadError>(&entry.value)? >= snapshot_position {
            return Ok(());
        }

        match bucket.update(key, value.clone().into(), entry.revision).await {
            Ok(_) => return Ok(()),
            Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
            Err(source) => {
                return Err(SnapshotKvError::UpdateSnapshotEntry { source }.into());
            }
        }
    }
}

async fn create_snapshot_value<PayloadError, SnapshotTypeError, K>(
    bucket: &K,
    key: &str,
    value: Vec<u8>,
) -> Result<bool, SnapshotStoreError<PayloadError, SnapshotTypeError>>
where
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvCreate,
{
    match bucket.create(key, value.into()).await {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => Ok(false),
        Err(source) => Err(SnapshotKvError::CreateSnapshotEntry { source }.into()),
    }
}

async fn delete_kv_value<PayloadError, SnapshotTypeError, K>(
    bucket: &K,
    key: &str,
) -> Result<(), SnapshotStoreError<PayloadError, SnapshotTypeError>>
where
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry + JetStreamKeyValueDeleteExpectRevision,
{
    loop {
        let Some(entry) = bucket
            .entry(key.to_string())
            .await
            .map_err(|source| SnapshotKvError::ReadEntryForDelete { source })?
        else {
            return Ok(());
        };
        if entry.operation != kv::Operation::Put {
            return Ok(());
        }

        match bucket.delete_expect_revision(key, Some(entry.revision)).await {
            Ok(()) => return Ok(()),
            Err(source) if source.kind() == kv::DeleteErrorKind::WrongLastRevision => continue,
            Err(source) => {
                return Err(SnapshotKvError::DeleteEntry { source }.into());
            }
        }
    }
}

#[cfg(test)]
mod tests;
