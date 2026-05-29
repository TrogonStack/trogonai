//! Snapshot storage helpers backed by JetStream Key/Value.
//!
//! Snapshots are stored under the [`SnapshotType::SNAPSHOT_STREAM_PREFIX`]
//! namespace for each payload type. Checkpoints are optional and use a reserved
//! `_snapshot.*` key so snapshot listings can separate stream snapshots from
//! adapter metadata.

use async_nats::jetstream::kv;
use bytes::Bytes;
use futures::StreamExt;
use std::{borrow::Cow, collections::BTreeMap, convert::Infallible};
use trogon_decider_runtime::StreamPosition;
use trogon_decider_runtime::snapshot::{
    EncodedSnapshot, Snapshot, SnapshotEnvelopeDecodeError, SnapshotEnvelopeEncodeError, SnapshotPayloadData,
    SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
};
use trogon_nats::jetstream::{
    JetStreamKeyValueDeleteExpectRevision, JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry,
    JetStreamKvGet, JetStreamKvKeys,
};

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

#[derive(Debug, Clone, Copy)]
enum SnapshotStoreContext {
    ListSnapshotKeys,
    ReadSnapshotKey,
    ReadSnapshotValue,
    ReadSnapshotEntry,
    AdvanceCheckpoint,
    CreateCheckpoint,
    ReadCheckpointEntry,
    DecodeCheckpoint,
    ReadEntryForUpdate,
    UpdateEntry,
    CreateEntry,
    ReadEntryForSnapshotUpdate,
    UpdateSnapshotEntry,
    CreateSnapshotEntry,
    ReadEntryForDelete,
    DeleteEntry,
}

impl SnapshotStoreContext {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ListSnapshotKeys => "failed to list stream snapshot keys",
            Self::ReadSnapshotKey => "failed to read stream snapshot key",
            Self::ReadSnapshotValue => "failed to read stream snapshot value",
            Self::ReadSnapshotEntry => "failed to read stream snapshot entry",
            Self::AdvanceCheckpoint => "failed to advance stream snapshot checkpoint",
            Self::CreateCheckpoint => "failed to create stream snapshot checkpoint",
            Self::ReadCheckpointEntry => "failed to read stream snapshot checkpoint entry",
            Self::DecodeCheckpoint => "failed to decode stream snapshot checkpoint",
            Self::ReadEntryForUpdate => "failed to read key-value entry for update",
            Self::UpdateEntry => "failed to update key-value entry",
            Self::CreateEntry => "failed to create key-value entry",
            Self::ReadEntryForSnapshotUpdate => "failed to read key-value entry for snapshot update",
            Self::UpdateSnapshotEntry => "failed to update snapshot entry",
            Self::CreateSnapshotEntry => "failed to create snapshot entry",
            Self::ReadEntryForDelete => "failed to read key-value entry for delete",
            Self::DeleteEntry => "failed to delete key-value entry",
        }
    }
}

#[derive(Debug)]
/// JetStream Key/Value error raised by snapshot storage.
#[allow(missing_docs)]
pub enum SnapshotKvError {
    ListSnapshotKeys { source: kv::HistoryError },
    ReadSnapshotKey { source: kv::WatcherError },
    ReadSnapshotValue { source: kv::EntryError },
    ReadSnapshotEntry { source: kv::EntryError },
    AdvanceCheckpoint { source: kv::UpdateError },
    CreateCheckpoint { source: kv::CreateError },
    ReadCheckpointEntry { source: kv::EntryError },
    DecodeCheckpoint { key: String },
    ReadEntryForUpdate { source: kv::EntryError },
    UpdateEntry { source: kv::UpdateError },
    CreateEntry { source: kv::CreateError },
    ReadEntryForSnapshotUpdate { source: kv::EntryError },
    UpdateSnapshotEntry { source: kv::UpdateError },
    CreateSnapshotEntry { source: kv::CreateError },
    ReadEntryForDelete { source: kv::EntryError },
    DeleteEntry { source: kv::DeleteError },
}

impl std::fmt::Display for SnapshotKvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ListSnapshotKeys { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::ListSnapshotKeys.as_str())
            }
            Self::ReadSnapshotKey { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::ReadSnapshotKey.as_str())
            }
            Self::ReadSnapshotValue { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::ReadSnapshotValue.as_str())
            }
            Self::ReadSnapshotEntry { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::ReadSnapshotEntry.as_str())
            }
            Self::AdvanceCheckpoint { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::AdvanceCheckpoint.as_str())
            }
            Self::CreateCheckpoint { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::CreateCheckpoint.as_str())
            }
            Self::ReadCheckpointEntry { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::ReadCheckpointEntry.as_str())
            }
            Self::DecodeCheckpoint { key } => write!(f, "{}: {key}", SnapshotStoreContext::DecodeCheckpoint.as_str()),
            Self::ReadEntryForUpdate { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::ReadEntryForUpdate.as_str())
            }
            Self::UpdateEntry { source } => write!(f, "{}: {source}", SnapshotStoreContext::UpdateEntry.as_str()),
            Self::CreateEntry { source } => write!(f, "{}: {source}", SnapshotStoreContext::CreateEntry.as_str()),
            Self::ReadEntryForSnapshotUpdate { source } => {
                write!(
                    f,
                    "{}: {source}",
                    SnapshotStoreContext::ReadEntryForSnapshotUpdate.as_str()
                )
            }
            Self::UpdateSnapshotEntry { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::UpdateSnapshotEntry.as_str())
            }
            Self::CreateSnapshotEntry { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::CreateSnapshotEntry.as_str())
            }
            Self::ReadEntryForDelete { source } => {
                write!(f, "{}: {source}", SnapshotStoreContext::ReadEntryForDelete.as_str())
            }
            Self::DeleteEntry { source } => write!(f, "{}: {source}", SnapshotStoreContext::DeleteEntry.as_str()),
        }
    }
}

impl std::error::Error for SnapshotKvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ListSnapshotKeys { source } => Some(source),
            Self::ReadSnapshotKey { source } => Some(source),
            Self::ReadSnapshotValue { source } => Some(source),
            Self::ReadSnapshotEntry { source } => Some(source),
            Self::AdvanceCheckpoint { source } => Some(source),
            Self::CreateCheckpoint { source } => Some(source),
            Self::ReadCheckpointEntry { source } => Some(source),
            Self::DecodeCheckpoint { .. } => None,
            Self::ReadEntryForUpdate { source } => Some(source),
            Self::UpdateEntry { source } => Some(source),
            Self::CreateEntry { source } => Some(source),
            Self::ReadEntryForSnapshotUpdate { source } => Some(source),
            Self::UpdateSnapshotEntry { source } => Some(source),
            Self::CreateSnapshotEntry { source } => Some(source),
            Self::ReadEntryForDelete { source } => Some(source),
            Self::DeleteEntry { source } => Some(source),
        }
    }
}

#[derive(Debug)]
/// Snapshot envelope or payload codec error.
#[allow(missing_docs)]
pub enum SnapshotCodecError<PayloadError> {
    EncodePayload { source: PayloadError },
    EncodeEnvelope { source: SnapshotEnvelopeEncodeError },
    DecodeEnvelope { source: SnapshotEnvelopeDecodeError },
    DecodePayload { source: PayloadError },
}

impl<PayloadError> std::fmt::Display for SnapshotCodecError<PayloadError>
where
    PayloadError: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EncodePayload { source } => write!(f, "failed to encode stream snapshot payload: {source}"),
            Self::EncodeEnvelope { source } => write!(f, "failed to encode stream snapshot envelope: {source}"),
            Self::DecodeEnvelope { source } => write!(f, "failed to decode stream snapshot envelope: {source}"),
            Self::DecodePayload { source } => write!(f, "failed to decode stream snapshot payload: {source}"),
        }
    }
}

impl<PayloadError> std::error::Error for SnapshotCodecError<PayloadError>
where
    PayloadError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EncodePayload { source } => Some(source),
            Self::EncodeEnvelope { source } => Some(source),
            Self::DecodeEnvelope { source } => Some(source),
            Self::DecodePayload { source } => Some(source),
        }
    }
}

#[derive(Debug)]
/// Error raised while reading, writing, or decoding snapshot state.
pub enum SnapshotStoreError<PayloadError = Infallible> {
    /// JetStream Key/Value operation failed.
    Kv(SnapshotKvError),
    /// Snapshot envelope or payload encoding failed.
    Codec(SnapshotCodecError<PayloadError>),
    /// A key used the snapshot namespace prefix but did not include a stream id.
    InvalidSnapshotKey {
        /// Invalid key observed in the Key/Value bucket.
        key: String,
    },
    /// Checkpoint operations were requested without a checkpoint name.
    MissingCheckpointName {
        /// Snapshot namespace prefix that requires a checkpoint name.
        key_prefix: String,
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
    /// Writes a snapshot for the stream id.
    Upsert {
        /// Domain stream id associated with the snapshot.
        stream_id: String,
        /// Snapshot payload and stream position to store.
        snapshot: Box<Snapshot<T>>,
    },
    /// Deletes a snapshot for the stream id.
    Delete {
        /// Domain stream id whose snapshot should be removed.
        stream_id: String,
    },
}

impl<T> SnapshotChange<T> {
    /// Builds an upsert change for a stream snapshot.
    pub fn upsert(stream_id: impl Into<String>, snapshot: Snapshot<T>) -> Self {
        Self::Upsert {
            stream_id: stream_id.into(),
            snapshot: Box::new(snapshot),
        }
    }

    /// Builds a delete change for a stream snapshot.
    pub fn delete(stream_id: impl Into<String>) -> Self {
        Self::Delete {
            stream_id: stream_id.into(),
        }
    }
}

impl<PayloadError> From<SnapshotKvError> for SnapshotStoreError<PayloadError> {
    fn from(source: SnapshotKvError) -> Self {
        Self::Kv(source)
    }
}

impl<PayloadError> From<SnapshotCodecError<PayloadError>> for SnapshotStoreError<PayloadError> {
    fn from(source: SnapshotCodecError<PayloadError>) -> Self {
        Self::Codec(source)
    }
}

impl<PayloadError> std::fmt::Display for SnapshotStoreError<PayloadError>
where
    PayloadError: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kv(source) => write!(f, "KV error: {source}"),
            Self::Codec(source) => write!(f, "Snapshot codec error: {source}"),
            Self::InvalidSnapshotKey { key } => {
                write!(f, "Invalid stream snapshot key: {key}")
            }
            Self::MissingCheckpointName { key_prefix } => {
                write!(f, "Missing checkpoint name for snapshot namespace: {key_prefix}")
            }
        }
    }
}

impl<PayloadError> std::error::Error for SnapshotStoreError<PayloadError>
where
    PayloadError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kv(source) => Some(source),
            Self::Codec(source) => Some(source),
            Self::InvalidSnapshotKey { .. } | Self::MissingCheckpointName { .. } => None,
        }
    }
}

/// Builds the Key/Value key used to store a stream snapshot.
pub fn snapshot_key<T>(id: &str) -> String
where
    T: SnapshotType,
{
    format!("{}{}", T::SNAPSHOT_STREAM_PREFIX, id)
}

/// Builds the Key/Value key used to store a snapshot checkpoint.
pub fn checkpoint_key<T>(config: &NatsSnapshotConfig) -> Result<String, SnapshotStoreError>
where
    T: SnapshotType,
{
    config
        .checkpoint_name()
        .map(|checkpoint_name| {
            format!(
                "_snapshot.{}.{}",
                T::SNAPSHOT_STREAM_PREFIX.trim_end_matches('.'),
                checkpoint_name
            )
        })
        .ok_or_else(|| SnapshotStoreError::MissingCheckpointName {
            key_prefix: T::SNAPSHOT_STREAM_PREFIX.to_string(),
        })
}

fn stream_id_from_snapshot_key<T, PayloadError>(key: &str) -> Result<Option<String>, SnapshotStoreError<PayloadError>>
where
    T: SnapshotType,
{
    let Some(stream_id) = key.strip_prefix(T::SNAPSHOT_STREAM_PREFIX) else {
        return Ok(None);
    };

    if stream_id.is_empty() {
        return Err(SnapshotStoreError::InvalidSnapshotKey { key: key.to_string() });
    }

    Ok(Some(stream_id.to_string()))
}

fn snapshot_value<T>(snapshot: &Snapshot<T>) -> Result<Vec<u8>, SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadEncode,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let payload = snapshot
        .payload
        .encode()
        .map_err(|source| SnapshotCodecError::EncodePayload { source })?;
    let encoded = EncodedSnapshot::new(snapshot.position, payload);

    encoded
        .into_bytes()
        .map_err(|source| SnapshotCodecError::EncodeEnvelope { source }.into())
}

fn snapshot_from_value<T>(value: &[u8]) -> Result<Snapshot<T>, SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadDecode,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let encoded = EncodedSnapshot::from_bytes(value).map_err(|source| SnapshotCodecError::DecodeEnvelope { source })?;
    let payload = T::decode(SnapshotPayloadData::new(encoded.payload.as_slice()))
        .map_err(|source| SnapshotCodecError::DecodePayload { source })?;

    Ok(Snapshot::new(encoded.position, payload))
}

fn snapshot_position_from_value<PayloadError>(value: &[u8]) -> Result<StreamPosition, SnapshotStoreError<PayloadError>>
where
    PayloadError: std::error::Error + Send + Sync + 'static,
{
    EncodedSnapshot::from_bytes(value)
        .map(|snapshot| snapshot.position)
        .map_err(|source| SnapshotCodecError::DecodeEnvelope { source }.into())
}

async fn read_snapshot_entries<T, K>(bucket: &K) -> Result<Vec<(String, Snapshot<T>)>, SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet + JetStreamKvKeys,
{
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| SnapshotKvError::ListSnapshotKeys { source })?;
    let mut snapshots = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| SnapshotKvError::ReadSnapshotKey { source })?;
        let Some(stream_id) = stream_id_from_snapshot_key::<T, T::Error>(&key)? else {
            continue;
        };
        let Some(value) = bucket
            .get(key)
            .await
            .map_err(|source| SnapshotKvError::ReadSnapshotValue { source })?
        else {
            continue;
        };
        let snapshot = snapshot_from_value::<T>(&value)?;
        snapshots.push((stream_id, snapshot));
    }

    Ok(snapshots)
}

/// Reads the snapshot currently stored for the stream id.
pub async fn read_snapshot<T, K>(bucket: &K, id: &str) -> Result<Option<Snapshot<T>>, SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet,
{
    let Some(value) = bucket
        .get(snapshot_key::<T>(id))
        .await
        .map_err(|source| SnapshotKvError::ReadSnapshotEntry { source })?
    else {
        return Ok(None);
    };

    snapshot_from_value::<T>(&value).map(Some)
}

/// Writes a snapshot for the stream id.
pub async fn write_snapshot<T, K>(
    bucket: &K,
    id: &str,
    snapshot: Snapshot<T>,
) -> Result<(), SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadEncode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
    K: SnapshotKvBucket,
{
    persist_snapshot_change(bucket, SnapshotChange::upsert(id, snapshot)).await
}

/// Lists snapshots in the payload type namespace.
pub async fn list_snapshots<T, K>(bucket: &K) -> Result<Vec<Snapshot<T>>, SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet + JetStreamKvKeys,
{
    read_snapshot_entries(bucket)
        .await
        .map(|entries| entries.into_iter().map(|(_, snapshot)| snapshot).collect())
}

/// Reads snapshots in the payload type namespace keyed by stream id.
pub async fn read_snapshot_map<T, K>(bucket: &K) -> Result<BTreeMap<String, Snapshot<T>>, SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvGet + JetStreamKvKeys,
{
    read_snapshot_entries(bucket)
        .await
        .map(|entries| entries.into_iter().collect())
}

/// Reads the configured checkpoint sequence.
///
/// Missing or deleted checkpoint entries are treated as sequence `0`.
pub async fn read_checkpoint<T, K>(bucket: &K, config: &NatsSnapshotConfig) -> Result<u64, SnapshotStoreError>
where
    T: SnapshotType,
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
) -> Result<(), SnapshotStoreError>
where
    T: SnapshotType,
    K: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    write_kv_value(bucket, &checkpoint_key::<T>(config)?, checkpoint_value(sequence)).await
}

/// Advances the checkpoint only when the current value is exactly one behind.
///
/// This keeps concurrent snapshot workers from skipping gaps or rewinding the
/// checkpoint while still allowing the winning writer to move it forward.
pub async fn maybe_advance_checkpoint<T, K>(
    bucket: &K,
    config: &NatsSnapshotConfig,
    sequence: u64,
) -> Result<(), SnapshotStoreError>
where
    T: SnapshotType,
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
) -> Result<(), SnapshotStoreError<T::Error>>
where
    T: SnapshotPayloadEncode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
    K: SnapshotKvBucket,
{
    match change {
        SnapshotChange::Upsert { stream_id, snapshot } => {
            let snapshot_position = snapshot.position;
            let value = snapshot_value(snapshot.as_ref())?;
            write_snapshot_value::<T::Error, K>(bucket, &snapshot_key::<T>(&stream_id), snapshot_position, value)
                .await?;
        }
        SnapshotChange::Delete { stream_id } => {
            delete_kv_value::<T::Error, K>(bucket, &snapshot_key::<T>(&stream_id)).await?;
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
) -> Result<(Option<u64>, u64), SnapshotStoreError>
where
    T: SnapshotType,
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

async fn write_kv_value<K>(bucket: &K, key: &str, value: Vec<u8>) -> Result<(), SnapshotStoreError>
where
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

async fn write_snapshot_value<PayloadError, K>(
    bucket: &K,
    key: &str,
    snapshot_position: StreamPosition,
    value: Vec<u8>,
) -> Result<(), SnapshotStoreError<PayloadError>>
where
    PayloadError: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    loop {
        let Some(entry) = bucket
            .entry(key.to_string())
            .await
            .map_err(|source| SnapshotKvError::ReadEntryForSnapshotUpdate { source })?
        else {
            if create_snapshot_value::<PayloadError, K>(bucket, key, value.clone()).await? {
                return Ok(());
            }
            continue;
        };

        if entry.operation != kv::Operation::Put {
            if create_snapshot_value::<PayloadError, K>(bucket, key, value.clone()).await? {
                return Ok(());
            }
            continue;
        }

        if snapshot_position_from_value::<PayloadError>(&entry.value)? >= snapshot_position {
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

async fn create_snapshot_value<PayloadError, K>(
    bucket: &K,
    key: &str,
    value: Vec<u8>,
) -> Result<bool, SnapshotStoreError<PayloadError>>
where
    PayloadError: std::error::Error + Send + Sync + 'static,
    K: JetStreamKvCreate,
{
    match bucket.create(key, value.into()).await {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => Ok(false),
        Err(source) => Err(SnapshotKvError::CreateSnapshotEntry { source }.into()),
    }
}

async fn delete_kv_value<PayloadError, K>(bucket: &K, key: &str) -> Result<(), SnapshotStoreError<PayloadError>>
where
    PayloadError: std::error::Error + Send + Sync + 'static,
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
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use trogon_decider_runtime::StreamPosition;
    use trogon_decider_runtime::snapshot::{
        SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
    };

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestPayload {
        id: String,
    }

    impl SnapshotType for TestPayload {
        const SNAPSHOT_STREAM_PREFIX: &'static str = "snapshots.v2.";
    }

    impl SnapshotPayloadEncode for TestPayload {
        type Error = serde_json::Error;

        fn encode(&self) -> Result<Vec<u8>, Self::Error> {
            serde_json::to_vec(self)
        }
    }

    impl SnapshotPayloadDecode for TestPayload {
        type Error = serde_json::Error;

        fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
            serde_json::from_slice(payload.payload)
        }
    }

    #[test]
    fn nats_snapshot_config_exposes_checkpoint_name() {
        let config = NatsSnapshotConfig::with_checkpoint_name("last_event_sequence");

        assert_eq!(config.checkpoint_name(), Some("last_event_sequence"));
    }

    #[test]
    fn snapshot_key_uses_snapshot_type_prefix() {
        assert_eq!(snapshot_key::<TestPayload>("backup"), "snapshots.v2.backup");
    }

    #[test]
    fn checkpoint_key_uses_nats_specific_format() {
        let config = NatsSnapshotConfig::with_checkpoint_name("last_event_sequence");

        assert_eq!(
            checkpoint_key::<TestPayload>(&config).unwrap(),
            "_snapshot.snapshots.v2.last_event_sequence"
        );
    }

    #[test]
    fn checkpoint_key_requires_configured_name() {
        let config = NatsSnapshotConfig::without_checkpoint();

        assert_eq!(
            checkpoint_key::<TestPayload>(&config).unwrap_err().to_string(),
            "Missing checkpoint name for snapshot namespace: snapshots.v2."
        );
    }

    #[test]
    fn snapshot_constructors_keep_position_and_payload() {
        let snapshot = Snapshot::new(
            position(9),
            TestPayload {
                id: "backup".to_string(),
            },
        );

        assert_eq!(snapshot.position, position(9));
        assert_eq!(snapshot.payload.id, "backup");
    }

    #[test]
    fn snapshot_change_builders_keep_stream_identity() {
        let upsert = SnapshotChange::upsert(
            "backup",
            Snapshot::new(
                position(3),
                TestPayload {
                    id: "backup".to_string(),
                },
            ),
        );
        let delete = SnapshotChange::<TestPayload>::delete("backup");

        assert_eq!(
            upsert,
            SnapshotChange::Upsert {
                stream_id: "backup".to_string(),
                snapshot: Box::new(Snapshot::new(
                    position(3),
                    TestPayload {
                        id: "backup".to_string(),
                    },
                )),
            }
        );
        assert_eq!(
            delete,
            SnapshotChange::Delete {
                stream_id: "backup".to_string(),
            }
        );
    }

    #[test]
    fn stored_snapshot_round_trips_with_nested_payload() {
        let snapshot = Snapshot::new(
            position(7),
            TestPayload {
                id: "backup".to_string(),
            },
        );

        let json = String::from_utf8(snapshot_value(&snapshot).unwrap()).unwrap();
        let decoded = snapshot_from_value::<TestPayload>(json.as_bytes()).unwrap();

        assert!(json.contains("\"position\":7"));
        assert!(json.contains("\"payload\""));
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn stream_id_from_snapshot_key_uses_configured_prefix() {
        assert_eq!(
            stream_id_from_snapshot_key::<TestPayload, serde_json::Error>("snapshots.v2.backup").unwrap(),
            Some("backup".to_string())
        );
        assert_eq!(
            stream_id_from_snapshot_key::<TestPayload, serde_json::Error>("snapshots.v1.backup").unwrap(),
            None
        );
    }

    #[test]
    fn stream_id_from_snapshot_key_rejects_empty_suffix() {
        assert_eq!(
            stream_id_from_snapshot_key::<TestPayload, serde_json::Error>("snapshots.v2.")
                .unwrap_err()
                .to_string(),
            "Invalid stream snapshot key: snapshots.v2."
        );
    }

    mod kv_operations {
        use super::*;
        use bytes::Bytes;
        use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

        fn snapshot_payload(id: &str) -> TestPayload {
            TestPayload { id: id.to_string() }
        }

        fn encoded_snapshot_at(position_value: u64, id: &str) -> Vec<u8> {
            snapshot_value(&Snapshot::new(position(position_value), snapshot_payload(id))).unwrap()
        }

        #[tokio::test]
        async fn write_snapshot_creates_entry_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();

            write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
                .await
                .expect("write should succeed");

            let calls = bucket.create_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "snapshots.v2.alpha");
        }

        #[tokio::test]
        async fn write_snapshot_skips_when_existing_position_is_at_or_above_new() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(10, "alpha")), 3, kv::Operation::Put);

            write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
                .await
                .expect("skip should be silent");

            assert!(bucket.update_calls().is_empty());
            assert!(bucket.create_calls().is_empty());
        }

        #[tokio::test]
        async fn write_snapshot_updates_when_existing_position_is_lower() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(3, "alpha")), 7, kv::Operation::Put);

            write_snapshot(&bucket, "alpha", Snapshot::new(position(9), snapshot_payload("alpha")))
                .await
                .expect("update should succeed");

            let updates = bucket.update_calls();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].0, "snapshots.v2.alpha");
            assert_eq!(updates[0].2, 7);
        }

        #[tokio::test]
        async fn write_snapshot_retries_on_wrong_last_revision() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(1, "alpha")), 4, kv::Operation::Put);
            bucket.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(2, "alpha")), 5, kv::Operation::Put);
            bucket.enqueue_update_result(Ok(6));

            write_snapshot(&bucket, "alpha", Snapshot::new(position(10), snapshot_payload("alpha")))
                .await
                .expect("write should succeed after retry");

            assert_eq!(bucket.update_calls().len(), 2);
            assert_eq!(bucket.entry_calls().len(), 2);
        }

        #[tokio::test]
        async fn write_snapshot_creates_when_entry_is_tombstone() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::new(), 4, kv::Operation::Delete);
            bucket.enqueue_create_result(Ok(5));

            write_snapshot(&bucket, "alpha", Snapshot::new(position(2), snapshot_payload("alpha")))
                .await
                .expect("write should succeed");

            assert_eq!(bucket.create_calls().len(), 1);
            assert!(bucket.update_calls().is_empty());
        }

        #[tokio::test]
        async fn write_snapshot_retries_when_create_already_exists() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            bucket.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(1, "alpha")), 2, kv::Operation::Put);
            bucket.enqueue_update_result(Ok(3));

            write_snapshot(&bucket, "alpha", Snapshot::new(position(5), snapshot_payload("alpha")))
                .await
                .expect("write should succeed after race");

            assert_eq!(bucket.create_calls().len(), 1);
            assert_eq!(bucket.update_calls().len(), 1);
        }

        #[tokio::test]
        async fn persist_snapshot_change_delete_is_idempotent_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();

            persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha"))
                .await
                .expect("delete should be idempotent");

            assert!(bucket.delete_calls().is_empty());
        }

        #[tokio::test]
        async fn persist_snapshot_change_delete_is_idempotent_on_tombstone() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::new(), 3, kv::Operation::Delete);

            persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha"))
                .await
                .expect("delete should be idempotent");

            assert!(bucket.delete_calls().is_empty());
        }

        #[tokio::test]
        async fn persist_snapshot_change_delete_retries_on_wrong_last_revision() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(1, "alpha")), 3, kv::Operation::Put);
            bucket.enqueue_delete_result(Err(kv::DeleteErrorKind::WrongLastRevision));
            bucket.enqueue_entry(Bytes::from(encoded_snapshot_at(2, "alpha")), 5, kv::Operation::Put);
            bucket.enqueue_delete_result(Ok(()));

            persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha"))
                .await
                .expect("delete should succeed after retry");

            assert_eq!(bucket.delete_calls().len(), 2);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_creates_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1)
                .await
                .expect("create should succeed");

            let calls = bucket.create_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "_snapshot.snapshots.v2.checkpoint");
            assert_eq!(calls[0].1, Bytes::from_static(b"1"));
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_updates_when_one_behind() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"4"), 9, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 5)
                .await
                .expect("update should succeed");

            let updates = bucket.update_calls();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].2, 9);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_is_noop_when_not_one_behind() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"3"), 7, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 7)
                .await
                .expect("noop should not error");

            assert!(bucket.update_calls().is_empty());
            assert!(bucket.create_calls().is_empty());
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_tolerates_wrong_last_revision() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"1"), 4, kv::Operation::Put);
            bucket.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 2)
                .await
                .expect("conflict should be silent");

            assert_eq!(bucket.update_calls().len(), 1);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_tolerates_already_exists() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            bucket.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1)
                .await
                .expect("race should be silent");

            assert_eq!(bucket.create_calls().len(), 1);
        }

        #[tokio::test]
        async fn maybe_advance_checkpoint_treats_tombstone_as_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::new(), 8, kv::Operation::Delete);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1)
                .await
                .expect("tombstone should create fresh checkpoint");

            assert_eq!(bucket.create_calls().len(), 1);
            assert!(bucket.update_calls().is_empty());
        }

        #[tokio::test]
        async fn read_checkpoint_returns_zero_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            let sequence = read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap();

            assert_eq!(sequence, 0);
        }

        #[tokio::test]
        async fn read_checkpoint_decodes_stored_sequence() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"42"), 1, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            let sequence = read_checkpoint::<TestPayload, _>(&bucket, &config).await.unwrap();

            assert_eq!(sequence, 42);
        }

        #[tokio::test]
        async fn write_checkpoint_creates_when_missing() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry_none();
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            write_checkpoint::<TestPayload, _>(&bucket, &config, 11)
                .await
                .expect("write should succeed");

            let calls = bucket.create_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].1, Bytes::from_static(b"11"));
        }

        #[tokio::test]
        async fn write_checkpoint_updates_when_present() {
            let bucket = MockJetStreamKvStore::new();
            bucket.enqueue_entry(Bytes::from_static(b"4"), 3, kv::Operation::Put);
            let config = NatsSnapshotConfig::with_checkpoint_name("checkpoint");

            write_checkpoint::<TestPayload, _>(&bucket, &config, 5)
                .await
                .expect("write should succeed");

            let updates = bucket.update_calls();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].1, Bytes::from_static(b"5"));
            assert_eq!(updates[0].2, 3);
        }

        #[tokio::test]
        async fn read_snapshot_entries_lists_keys_and_decodes_values() {
            let bucket = MockJetStreamKvStore::new();
            bucket.set_keys_result(Ok(vec![
                "snapshots.v2.alpha".to_string(),
                "snapshots.v1.legacy".to_string(),
                "snapshots.v2.beta".to_string(),
            ]));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(3, "alpha")));
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(7, "beta")));

            let entries = list_snapshots::<TestPayload, _>(&bucket).await.unwrap();

            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].position, position(3));
            assert_eq!(entries[1].position, position(7));
            assert_eq!(bucket.keys_calls(), 1);
        }

        #[tokio::test]
        async fn read_snapshot_entries_skips_missing_values() {
            let bucket = MockJetStreamKvStore::new();
            bucket.set_keys_result(Ok(vec![
                "snapshots.v2.alpha".to_string(),
                "snapshots.v2.beta".to_string(),
            ]));
            bucket.enqueue_get_none();
            bucket.enqueue_get_some(Bytes::from(encoded_snapshot_at(2, "beta")));

            let entries = list_snapshots::<TestPayload, _>(&bucket).await.unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].position, position(2));
        }
    }
}
