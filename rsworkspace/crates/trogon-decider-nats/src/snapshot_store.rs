//! Snapshot storage helpers backed by JetStream Key/Value.
//!
//! Snapshots are stored under the [`SnapshotType::SNAPSHOT_STREAM_PREFIX`]
//! namespace for each payload type. Checkpoints are optional and use a reserved
//! `_snapshot.*` key so snapshot listings can separate stream snapshots from
//! adapter metadata.

use async_nats::jetstream::kv;
use bytes::Bytes;
use futures::StreamExt;
use std::{borrow::Cow, collections::BTreeMap};
use trogon_decider_runtime::StreamPosition;
use trogon_decider_runtime::snapshot::{
    EncodedSnapshot, Snapshot, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType, decode_snapshot,
    encode_snapshot,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
/// Error raised while reading, writing, or decoding snapshot state.
pub enum SnapshotStoreError {
    /// JetStream Key/Value operation failed.
    Kv {
        /// Operation context for the failed Key/Value call.
        context: &'static str,
        /// Source error returned by the Key/Value client.
        source: BoxError,
    },
    /// Snapshot envelope or payload encoding failed.
    Codec {
        /// Operation context for the failed codec call.
        context: &'static str,
        /// Source error returned by the codec.
        source: BoxError,
    },
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

impl SnapshotStoreError {
    fn kv_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Kv {
            context,
            source: Box::new(source),
        }
    }

    fn codec_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Codec {
            context,
            source: Box::new(source),
        }
    }
}

impl std::fmt::Display for SnapshotStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kv { context, source } => write!(f, "KV error: {context}: {source}"),
            Self::Codec { context, source } => write!(f, "Snapshot codec error: {context}: {source}"),
            Self::InvalidSnapshotKey { key } => {
                write!(f, "Invalid stream snapshot key: {key}")
            }
            Self::MissingCheckpointName { key_prefix } => {
                write!(f, "Missing checkpoint name for snapshot namespace: {key_prefix}")
            }
        }
    }
}

impl std::error::Error for SnapshotStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kv { source, .. } | Self::Codec { source, .. } => Some(source.as_ref()),
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

fn stream_id_from_snapshot_key<T>(key: &str) -> Result<Option<String>, SnapshotStoreError>
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

fn snapshot_value<T>(snapshot: &Snapshot<T>) -> Result<Vec<u8>, SnapshotStoreError>
where
    T: SnapshotPayloadEncode,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let encoded = encode_snapshot(snapshot)
        .map_err(|source| SnapshotStoreError::codec_source("failed to encode stream snapshot payload", source))?;

    encoded
        .into_bytes()
        .map_err(|source| SnapshotStoreError::codec_source("failed to encode stream snapshot envelope", source))
}

fn snapshot_from_value<T>(value: &[u8]) -> Result<Snapshot<T>, SnapshotStoreError>
where
    T: SnapshotPayloadDecode,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let encoded = EncodedSnapshot::from_bytes(value)
        .map_err(|source| SnapshotStoreError::codec_source("failed to decode stream snapshot envelope", source))?;

    decode_snapshot(encoded)
        .map_err(|source| SnapshotStoreError::codec_source("failed to decode stream snapshot payload", source))
}

fn snapshot_position_from_value(value: &[u8]) -> Result<StreamPosition, SnapshotStoreError> {
    EncodedSnapshot::from_bytes(value)
        .map(|snapshot| snapshot.position)
        .map_err(|source| SnapshotStoreError::codec_source("failed to decode stream snapshot envelope", source))
}

async fn read_snapshot_entries<T>(bucket: &kv::Store) -> Result<Vec<(String, Snapshot<T>)>, SnapshotStoreError>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| SnapshotStoreError::kv_source("failed to list stream snapshot keys", source))?;
    let mut snapshots = Vec::new();

    while let Some(result) = keys.next().await {
        let key =
            result.map_err(|source| SnapshotStoreError::kv_source("failed to read stream snapshot key", source))?;
        let Some(stream_id) = stream_id_from_snapshot_key::<T>(&key)? else {
            continue;
        };
        let Some(value) = bucket
            .get(key)
            .await
            .map_err(|source| SnapshotStoreError::kv_source("failed to read stream snapshot value", source))?
        else {
            continue;
        };
        let snapshot = snapshot_from_value::<T>(&value)?;
        snapshots.push((stream_id, snapshot));
    }

    Ok(snapshots)
}

/// Reads the snapshot currently stored for the stream id.
pub async fn read_snapshot<T>(bucket: &kv::Store, id: &str) -> Result<Option<Snapshot<T>>, SnapshotStoreError>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let Some(value) = bucket
        .get(snapshot_key::<T>(id))
        .await
        .map_err(|source| SnapshotStoreError::kv_source("failed to read stream snapshot entry", source))?
    else {
        return Ok(None);
    };

    snapshot_from_value::<T>(&value).map(Some)
}

/// Writes a snapshot for the stream id.
pub async fn write_snapshot<T>(bucket: &kv::Store, id: &str, snapshot: Snapshot<T>) -> Result<(), SnapshotStoreError>
where
    T: SnapshotPayloadEncode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    persist_snapshot_change(bucket, SnapshotChange::upsert(id, snapshot)).await
}

/// Lists snapshots in the payload type namespace.
pub async fn list_snapshots<T>(bucket: &kv::Store) -> Result<Vec<Snapshot<T>>, SnapshotStoreError>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    read_snapshot_entries(bucket)
        .await
        .map(|entries| entries.into_iter().map(|(_, snapshot)| snapshot).collect())
}

/// Reads snapshots in the payload type namespace keyed by stream id.
pub async fn read_snapshot_map<T>(bucket: &kv::Store) -> Result<BTreeMap<String, Snapshot<T>>, SnapshotStoreError>
where
    T: SnapshotPayloadDecode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    read_snapshot_entries(bucket)
        .await
        .map(|entries| entries.into_iter().collect())
}

/// Reads the configured checkpoint sequence.
///
/// Missing or deleted checkpoint entries are treated as sequence `0`.
pub async fn read_checkpoint<T>(bucket: &kv::Store, config: &NatsSnapshotConfig) -> Result<u64, SnapshotStoreError>
where
    T: SnapshotType,
{
    let (_revision, sequence) = read_checkpoint_entry::<T>(bucket, config).await?;
    Ok(sequence)
}

/// Writes the configured checkpoint sequence unconditionally.
pub async fn write_checkpoint<T>(
    bucket: &kv::Store,
    config: &NatsSnapshotConfig,
    sequence: u64,
) -> Result<(), SnapshotStoreError>
where
    T: SnapshotType,
{
    write_kv_value(bucket, &checkpoint_key::<T>(config)?, checkpoint_value(sequence)).await
}

/// Advances the checkpoint only when the current value is exactly one behind.
///
/// This keeps concurrent snapshot workers from skipping gaps or rewinding the
/// checkpoint while still allowing the winning writer to move it forward.
pub async fn maybe_advance_checkpoint<T>(
    bucket: &kv::Store,
    config: &NatsSnapshotConfig,
    sequence: u64,
) -> Result<(), SnapshotStoreError>
where
    T: SnapshotType,
{
    let expected_previous = sequence.saturating_sub(1);
    let (revision, current_sequence) = read_checkpoint_entry::<T>(bucket, config).await?;
    if current_sequence != expected_previous {
        return Ok(());
    }

    let checkpoint_key = checkpoint_key::<T>(config)?;
    let value = checkpoint_value(sequence);
    match revision {
        Some(revision) => match bucket.update(&checkpoint_key, value.into(), revision).await {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(()),
            Err(source) => Err(SnapshotStoreError::kv_source(
                "failed to advance stream snapshot checkpoint",
                source,
            )),
        },
        None => match bucket.create(&checkpoint_key, value.into()).await {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => Ok(()),
            Err(source) => Err(SnapshotStoreError::kv_source(
                "failed to create stream snapshot checkpoint",
                source,
            )),
        },
    }
}

/// Persists a snapshot mutation.
///
/// Upserts keep the highest snapshot position already present in the bucket;
/// deletes are idempotent.
pub async fn persist_snapshot_change<T>(bucket: &kv::Store, change: SnapshotChange<T>) -> Result<(), SnapshotStoreError>
where
    T: SnapshotPayloadEncode + SnapshotType,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    match change {
        SnapshotChange::Upsert { stream_id, snapshot } => {
            let snapshot_position = snapshot.position;
            let value = snapshot_value(snapshot.as_ref())?;
            write_snapshot_value(bucket, &snapshot_key::<T>(&stream_id), snapshot_position, value).await?;
        }
        SnapshotChange::Delete { stream_id } => {
            delete_kv_value(bucket, &snapshot_key::<T>(&stream_id)).await?;
        }
    }

    Ok(())
}

fn checkpoint_value(sequence: u64) -> Vec<u8> {
    sequence.to_string().into_bytes()
}

async fn read_checkpoint_entry<T>(
    bucket: &kv::Store,
    config: &NatsSnapshotConfig,
) -> Result<(Option<u64>, u64), SnapshotStoreError>
where
    T: SnapshotType,
{
    let checkpoint_key = checkpoint_key::<T>(config)?;
    let Some(entry) = bucket
        .entry(checkpoint_key.clone())
        .await
        .map_err(|source| SnapshotStoreError::kv_source("failed to read stream snapshot checkpoint entry", source))?
    else {
        return Ok((None, 0));
    };
    if entry.operation != kv::Operation::Put {
        return Ok((None, 0));
    }

    let sequence = String::from_utf8(entry.value.to_vec())
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            SnapshotStoreError::kv_source(
                "failed to decode stream snapshot checkpoint",
                std::io::Error::other(checkpoint_key),
            )
        })?;

    Ok((Some(entry.revision), sequence))
}

async fn write_kv_value(bucket: &kv::Store, key: &str, value: Vec<u8>) -> Result<(), SnapshotStoreError> {
    let value = Bytes::from(value);
    loop {
        if let Some(entry) = bucket
            .entry(key.to_string())
            .await
            .map_err(|source| SnapshotStoreError::kv_source("failed to read key-value entry for update", source))?
        {
            match bucket.update(key, value.clone(), entry.revision).await {
                Ok(_) => return Ok(()),
                Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(source) => {
                    return Err(SnapshotStoreError::kv_source(
                        "failed to update key-value entry",
                        source,
                    ));
                }
            }
        } else {
            match bucket.create(key, value.clone()).await {
                Ok(_) => return Ok(()),
                Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(SnapshotStoreError::kv_source(
                        "failed to create key-value entry",
                        source,
                    ));
                }
            }
        }
    }
}

async fn write_snapshot_value(
    bucket: &kv::Store,
    key: &str,
    snapshot_position: StreamPosition,
    value: Vec<u8>,
) -> Result<(), SnapshotStoreError> {
    loop {
        let Some(entry) = bucket.entry(key.to_string()).await.map_err(|source| {
            SnapshotStoreError::kv_source("failed to read key-value entry for snapshot update", source)
        })?
        else {
            if create_snapshot_value(bucket, key, value.clone()).await? {
                return Ok(());
            }
            continue;
        };

        if entry.operation != kv::Operation::Put {
            if create_snapshot_value(bucket, key, value.clone()).await? {
                return Ok(());
            }
            continue;
        }

        if snapshot_position_from_value(&entry.value)? >= snapshot_position {
            return Ok(());
        }

        match bucket.update(key, value.clone().into(), entry.revision).await {
            Ok(_) => return Ok(()),
            Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
            Err(source) => return Err(SnapshotStoreError::kv_source("failed to update snapshot entry", source)),
        }
    }
}

async fn create_snapshot_value(bucket: &kv::Store, key: &str, value: Vec<u8>) -> Result<bool, SnapshotStoreError> {
    match bucket.create(key, value.into()).await {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => Ok(false),
        Err(source) => Err(SnapshotStoreError::kv_source("failed to create snapshot entry", source)),
    }
}

async fn delete_kv_value(bucket: &kv::Store, key: &str) -> Result<(), SnapshotStoreError> {
    loop {
        let Some(entry) = bucket
            .entry(key.to_string())
            .await
            .map_err(|source| SnapshotStoreError::kv_source("failed to read key-value entry for delete", source))?
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
                return Err(SnapshotStoreError::kv_source(
                    "failed to delete key-value entry",
                    source,
                ));
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
            stream_id_from_snapshot_key::<TestPayload>("snapshots.v2.backup").unwrap(),
            Some("backup".to_string())
        );
        assert_eq!(
            stream_id_from_snapshot_key::<TestPayload>("snapshots.v1.backup").unwrap(),
            None
        );
    }

    #[test]
    fn stream_id_from_snapshot_key_rejects_empty_suffix() {
        assert_eq!(
            stream_id_from_snapshot_key::<TestPayload>("snapshots.v2.")
                .unwrap_err()
                .to_string(),
            "Invalid stream snapshot key: snapshots.v2."
        );
    }
}
