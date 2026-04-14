use async_nats::jetstream::kv;
use futures::StreamExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot<T> {
    pub version: u64,
    pub payload: T,
}

impl<T> Snapshot<T> {
    pub fn new(version: u64, payload: T) -> Self {
        Self { version, payload }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotStoreConfig<'a> {
    key_prefix: &'a str,
    checkpoint_key: &'a str,
}

impl<'a> SnapshotStoreConfig<'a> {
    pub const fn new(key_prefix: &'a str, checkpoint_key: &'a str) -> Self {
        Self {
            key_prefix,
            checkpoint_key,
        }
    }

    pub const fn key_prefix(self) -> &'a str {
        self.key_prefix
    }

    pub const fn checkpoint_key(self) -> &'a str {
        self.checkpoint_key
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotChange<T> {
    Upsert {
        stream_id: String,
        snapshot: Box<Snapshot<T>>,
    },
    Delete {
        stream_id: String,
    },
}

impl<T> SnapshotChange<T> {
    pub fn upsert(stream_id: impl Into<String>, snapshot: Snapshot<T>) -> Self {
        Self::Upsert {
            stream_id: stream_id.into(),
            snapshot: Box::new(snapshot),
        }
    }

    pub fn delete(stream_id: impl Into<String>) -> Self {
        Self::Delete {
            stream_id: stream_id.into(),
        }
    }
}

#[derive(Debug)]
pub enum SnapshotStoreError {
    Kv {
        context: &'static str,
        source: BoxError,
    },
    InvalidSnapshotKey {
        key: String,
    },
    Serde(serde_json::Error),
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
}

impl std::fmt::Display for SnapshotStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kv { context, source } => write!(f, "KV error: {context}: {source}"),
            Self::InvalidSnapshotKey { key } => {
                write!(f, "Invalid stream snapshot key: {key}")
            }
            Self::Serde(source) => write!(f, "Serialization error: {source}"),
        }
    }
}

impl std::error::Error for SnapshotStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kv { source, .. } => Some(source.as_ref()),
            Self::InvalidSnapshotKey { .. } => None,
            Self::Serde(source) => Some(source),
        }
    }
}

impl From<serde_json::Error> for SnapshotStoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value)
    }
}

pub fn snapshot_key(config: SnapshotStoreConfig<'_>, id: &str) -> String {
    format!("{}{}", config.key_prefix(), id)
}

pub fn checkpoint_key(config: SnapshotStoreConfig<'_>) -> String {
    config.checkpoint_key().to_string()
}

fn snapshot_key_prefix(config: SnapshotStoreConfig<'_>) -> String {
    config.key_prefix().to_string()
}

fn stream_id_from_snapshot_key(
    config: SnapshotStoreConfig<'_>,
    key: &str,
) -> Result<Option<String>, SnapshotStoreError> {
    let prefix = snapshot_key_prefix(config);
    let Some(stream_id) = key.strip_prefix(&prefix) else {
        return Ok(None);
    };

    if stream_id.is_empty() {
        return Err(SnapshotStoreError::InvalidSnapshotKey {
            key: key.to_string(),
        });
    }

    Ok(Some(stream_id.to_string()))
}

async fn load_snapshot_entries<T>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
) -> Result<Vec<(String, Snapshot<T>)>, SnapshotStoreError>
where
    T: DeserializeOwned,
{
    let mut keys = bucket.keys().await.map_err(|source| {
        SnapshotStoreError::kv_source("failed to list stream snapshot keys", source)
    })?;
    let mut snapshots = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| {
            SnapshotStoreError::kv_source("failed to read stream snapshot key", source)
        })?;
        let Some(stream_id) = stream_id_from_snapshot_key(config, &key)? else {
            continue;
        };
        let Some(entry) = bucket.entry(key).await.map_err(|source| {
            SnapshotStoreError::kv_source("failed to read stream snapshot value", source)
        })?
        else {
            continue;
        };
        let snapshot = serde_json::from_slice::<Snapshot<T>>(&entry.value).map_err(|source| {
            SnapshotStoreError::kv_source("failed to decode stream snapshot value", source)
        })?;
        snapshots.push((stream_id, snapshot));
    }

    Ok(snapshots)
}

pub async fn load_snapshot<T>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
    id: &str,
) -> Result<Option<Snapshot<T>>, SnapshotStoreError>
where
    T: DeserializeOwned,
{
    let Some(entry) = bucket
        .entry(snapshot_key(config, id))
        .await
        .map_err(|source| {
            SnapshotStoreError::kv_source("failed to read stream snapshot entry", source)
        })?
    else {
        return Ok(None);
    };

    serde_json::from_slice::<Snapshot<T>>(&entry.value)
        .map(Some)
        .map_err(|source| {
            SnapshotStoreError::kv_source("failed to decode stream snapshot entry", source)
        })
}

pub async fn list_snapshots<T>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
) -> Result<Vec<Snapshot<T>>, SnapshotStoreError>
where
    T: DeserializeOwned,
{
    load_snapshot_entries(bucket, config)
        .await
        .map(|entries| entries.into_iter().map(|(_, snapshot)| snapshot).collect())
}

pub async fn load_snapshot_map<T>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
) -> Result<BTreeMap<String, Snapshot<T>>, SnapshotStoreError>
where
    T: DeserializeOwned,
{
    load_snapshot_entries(bucket, config)
        .await
        .map(|entries| entries.into_iter().collect())
}

pub async fn read_checkpoint(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
) -> Result<u64, SnapshotStoreError> {
    let (_revision, sequence) = read_checkpoint_entry(bucket, config).await?;
    Ok(sequence)
}

pub async fn write_checkpoint(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
    sequence: u64,
) -> Result<(), SnapshotStoreError> {
    write_kv_value(bucket, &checkpoint_key(config), checkpoint_value(sequence)).await
}

pub async fn maybe_advance_checkpoint(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
    sequence: u64,
) -> Result<(), SnapshotStoreError> {
    let expected_previous = sequence.saturating_sub(1);
    let (revision, current_sequence) = read_checkpoint_entry(bucket, config).await?;
    if current_sequence != expected_previous {
        return Ok(());
    }

    let value = checkpoint_value(sequence);
    match revision {
        Some(revision) => match bucket
            .update(&checkpoint_key(config), value.into(), revision)
            .await
        {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => Ok(()),
            Err(source) => Err(SnapshotStoreError::kv_source(
                "failed to advance stream snapshot checkpoint",
                source,
            )),
        },
        None => match bucket.create(&checkpoint_key(config), value.into()).await {
            Ok(_) => Ok(()),
            Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => Ok(()),
            Err(source) => Err(SnapshotStoreError::kv_source(
                "failed to create stream snapshot checkpoint",
                source,
            )),
        },
    }
}

pub async fn persist_snapshot_change<T>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
    change: SnapshotChange<T>,
) -> Result<(), SnapshotStoreError>
where
    T: Serialize,
{
    match change {
        SnapshotChange::Upsert {
            stream_id,
            snapshot,
        } => {
            let value = serde_json::to_vec(snapshot.as_ref())?;
            write_kv_value(bucket, &snapshot_key(config, &stream_id), value).await?;
        }
        SnapshotChange::Delete { stream_id } => {
            delete_kv_value(bucket, &snapshot_key(config, &stream_id)).await?;
        }
    }

    Ok(())
}

fn checkpoint_value(sequence: u64) -> Vec<u8> {
    sequence.to_string().into_bytes()
}

async fn read_checkpoint_entry(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
) -> Result<(Option<u64>, u64), SnapshotStoreError> {
    let Some(entry) = bucket
        .entry(checkpoint_key(config))
        .await
        .map_err(|source| {
            SnapshotStoreError::kv_source("failed to read stream snapshot checkpoint entry", source)
        })?
    else {
        return Ok((None, 0));
    };

    let sequence = String::from_utf8(entry.value.to_vec())
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            SnapshotStoreError::kv_source(
                "failed to decode stream snapshot checkpoint",
                std::io::Error::other(checkpoint_key(config)),
            )
        })?;

    Ok((Some(entry.revision), sequence))
}

async fn write_kv_value(
    bucket: &kv::Store,
    key: &str,
    value: Vec<u8>,
) -> Result<(), SnapshotStoreError> {
    if let Some(entry) = bucket.entry(key.to_string()).await.map_err(|source| {
        SnapshotStoreError::kv_source("failed to read key-value entry for update", source)
    })? {
        let _ = bucket
            .update(key, value.into(), entry.revision)
            .await
            .map_err(|source| {
                SnapshotStoreError::kv_source("failed to update key-value entry", source)
            })?;
    } else {
        let _ = bucket.create(key, value.into()).await.map_err(|source| {
            SnapshotStoreError::kv_source("failed to create key-value entry", source)
        })?;
    }

    Ok(())
}

async fn delete_kv_value(bucket: &kv::Store, key: &str) -> Result<(), SnapshotStoreError> {
    if let Some(entry) = bucket.entry(key.to_string()).await.map_err(|source| {
        SnapshotStoreError::kv_source("failed to read key-value entry for delete", source)
    })? {
        bucket
            .delete_expect_revision(key, Some(entry.revision))
            .await
            .map_err(|source| {
                SnapshotStoreError::kv_source("failed to delete key-value entry", source)
            })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestPayload {
        id: String,
    }

    #[test]
    fn snapshot_store_config_exposes_values() {
        let config = SnapshotStoreConfig::new("snapshots.v2.", "_checkpoint.v2");

        assert_eq!(config.key_prefix(), "snapshots.v2.");
        assert_eq!(config.checkpoint_key(), "_checkpoint.v2");
    }

    #[test]
    fn snapshot_key_uses_prefix() {
        let config = SnapshotStoreConfig::new("snapshots.v2.", "_checkpoint.v2");

        assert_eq!(snapshot_key(config, "backup"), "snapshots.v2.backup");
    }

    #[test]
    fn checkpoint_key_uses_config_value() {
        let config = SnapshotStoreConfig::new("snapshots.v3.", "_checkpoint.v3");

        assert_eq!(checkpoint_key(config), "_checkpoint.v3");
    }

    #[test]
    fn snapshot_constructors_keep_version_and_payload() {
        let snapshot = Snapshot::new(
            9,
            TestPayload {
                id: "backup".to_string(),
            },
        );

        assert_eq!(snapshot.version, 9);
        assert_eq!(snapshot.payload.id, "backup");
    }

    #[test]
    fn snapshot_change_builders_keep_stream_identity() {
        let upsert = SnapshotChange::upsert(
            "backup",
            Snapshot::new(
                3,
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
                    3,
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
    fn snapshot_round_trips_with_nested_payload() {
        let snapshot = Snapshot::new(
            7,
            TestPayload {
                id: "backup".to_string(),
            },
        );

        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: Snapshot<TestPayload> = serde_json::from_str(&json).unwrap();

        assert!(json.contains("\"version\":7"));
        assert!(json.contains("\"payload\""));
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn stream_id_from_snapshot_key_uses_configured_prefix() {
        let config = SnapshotStoreConfig::new("snapshots.v2.", "_checkpoint.v2");

        assert_eq!(
            stream_id_from_snapshot_key(config, "snapshots.v2.backup").unwrap(),
            Some("backup".to_string())
        );
        assert_eq!(
            stream_id_from_snapshot_key(config, "snapshots.v1.backup").unwrap(),
            None
        );
    }

    #[test]
    fn stream_id_from_snapshot_key_rejects_empty_suffix() {
        let config = SnapshotStoreConfig::new("snapshots.v2.", "_checkpoint.v2");

        assert_eq!(
            stream_id_from_snapshot_key(config, "snapshots.v2.")
                .unwrap_err()
                .to_string(),
            "Invalid stream snapshot key: snapshots.v2."
        );
    }
}
