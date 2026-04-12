use std::collections::BTreeMap;
use std::num::NonZeroU16;

use async_nats::jetstream::kv;
use futures::StreamExt;
use serde::{Serialize, de::DeserializeOwned};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub trait StreamSnapshot {
    fn stream_id(&self) -> &str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotSchemaVersion(NonZeroU16);

impl SnapshotSchemaVersion {
    pub const V1: Self = Self(NonZeroU16::MIN);

    pub const fn new(value: NonZeroU16) -> Self {
        Self(value)
    }

    pub const fn as_u16(self) -> u16 {
        self.0.get()
    }
}

impl TryFrom<u16> for SnapshotSchemaVersion {
    type Error = &'static str;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        let Some(value) = NonZeroU16::new(value) else {
            return Err("snapshot schema version must be >= 1");
        };

        Ok(Self::new(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotStoreConfig<'a> {
    key_prefix: &'a str,
    checkpoint_key: &'a str,
    schema_version: SnapshotSchemaVersion,
}

impl<'a> SnapshotStoreConfig<'a> {
    pub const fn new(
        key_prefix: &'a str,
        checkpoint_key: &'a str,
        schema_version: SnapshotSchemaVersion,
    ) -> Self {
        Self {
            key_prefix,
            checkpoint_key,
            schema_version,
        }
    }

    pub const fn key_prefix(self) -> &'a str {
        self.key_prefix
    }

    pub const fn checkpoint_key(self) -> &'a str {
        self.checkpoint_key
    }

    pub const fn schema_version(self) -> SnapshotSchemaVersion {
        self.schema_version
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotChange<S> {
    Upsert(Box<S>),
    Delete(String),
}

impl<S> SnapshotChange<S> {
    pub fn upsert(snapshot: S) -> Self {
        Self::Upsert(Box::new(snapshot))
    }

    pub fn delete(id: impl Into<String>) -> Self {
        Self::Delete(id.into())
    }
}

#[derive(Debug)]
pub enum SnapshotStoreError {
    Kv {
        context: &'static str,
        source: BoxError,
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
            Self::Serde(source) => write!(f, "Serialization error: {source}"),
        }
    }
}

impl std::error::Error for SnapshotStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kv { source, .. } => Some(source.as_ref()),
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
    format!(
        "{}v{}.{}",
        config.key_prefix(),
        config.schema_version().as_u16(),
        id
    )
}

pub fn checkpoint_key(config: SnapshotStoreConfig<'_>) -> String {
    format!(
        "{}.v{}",
        config.checkpoint_key(),
        config.schema_version().as_u16()
    )
}

fn snapshot_key_prefix(config: SnapshotStoreConfig<'_>) -> String {
    format!(
        "{}v{}.",
        config.key_prefix(),
        config.schema_version().as_u16()
    )
}

pub async fn load_snapshot<S>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
    id: &str,
) -> Result<Option<S>, SnapshotStoreError>
where
    S: DeserializeOwned,
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

    serde_json::from_slice::<S>(&entry.value)
        .map(Some)
        .map_err(|source| {
            SnapshotStoreError::kv_source("failed to decode stream snapshot entry", source)
        })
}

pub async fn list_snapshots<S>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
) -> Result<Vec<S>, SnapshotStoreError>
where
    S: DeserializeOwned,
{
    let mut keys = bucket.keys().await.map_err(|source| {
        SnapshotStoreError::kv_source("failed to list stream snapshot keys", source)
    })?;
    let mut snapshots = Vec::new();

    while let Some(result) = keys.next().await {
        let key = result.map_err(|source| {
            SnapshotStoreError::kv_source("failed to read stream snapshot key", source)
        })?;
        if !key.starts_with(&snapshot_key_prefix(config)) {
            continue;
        }
        let Some(entry) = bucket.entry(key).await.map_err(|source| {
            SnapshotStoreError::kv_source("failed to read stream snapshot value", source)
        })?
        else {
            continue;
        };
        let snapshot = serde_json::from_slice::<S>(&entry.value).map_err(|source| {
            SnapshotStoreError::kv_source("failed to decode stream snapshot value", source)
        })?;
        snapshots.push(snapshot);
    }

    Ok(snapshots)
}

pub async fn load_snapshot_map<S>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
) -> Result<BTreeMap<String, S>, SnapshotStoreError>
where
    S: StreamSnapshot + DeserializeOwned,
{
    let snapshots: Vec<S> = list_snapshots(bucket, config).await?;
    let mut snapshot_map = BTreeMap::new();

    for snapshot in snapshots {
        snapshot_map.insert(snapshot.stream_id().to_string(), snapshot);
    }

    Ok(snapshot_map)
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

pub async fn persist_snapshot_change<S>(
    bucket: &kv::Store,
    config: SnapshotStoreConfig<'_>,
    change: SnapshotChange<S>,
) -> Result<(), SnapshotStoreError>
where
    S: StreamSnapshot + Serialize,
{
    match change {
        SnapshotChange::Upsert(snapshot) => {
            let value = serde_json::to_vec(snapshot.as_ref())?;
            write_kv_value(bucket, &snapshot_key(config, snapshot.stream_id()), value).await?;
        }
        SnapshotChange::Delete(id) => {
            delete_kv_value(bucket, &snapshot_key(config, &id)).await?;
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

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestSnapshot {
        id: String,
    }

    impl StreamSnapshot for TestSnapshot {
        fn stream_id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn snapshot_store_config_exposes_values() {
        let config = SnapshotStoreConfig::new(
            "snapshots.",
            "_checkpoint",
            SnapshotSchemaVersion::try_from(2).unwrap(),
        );

        assert_eq!(config.key_prefix(), "snapshots.");
        assert_eq!(config.checkpoint_key(), "_checkpoint");
        assert_eq!(config.schema_version().as_u16(), 2);
    }

    #[test]
    fn snapshot_key_uses_prefix() {
        let config = SnapshotStoreConfig::new(
            "snapshots.",
            "_checkpoint",
            SnapshotSchemaVersion::try_from(2).unwrap(),
        );

        assert_eq!(snapshot_key(config, "backup"), "snapshots.v2.backup");
    }

    #[test]
    fn checkpoint_key_uses_schema_version() {
        let config = SnapshotStoreConfig::new(
            "snapshots.",
            "_checkpoint",
            SnapshotSchemaVersion::try_from(3).unwrap(),
        );

        assert_eq!(checkpoint_key(config), "_checkpoint.v3");
    }

    #[test]
    fn snapshot_schema_version_rejects_zero() {
        assert_eq!(
            SnapshotSchemaVersion::try_from(0),
            Err("snapshot schema version must be >= 1")
        );
    }

    #[test]
    fn snapshot_change_builders_keep_payloads() {
        let upsert = SnapshotChange::upsert(TestSnapshot {
            id: "backup".to_string(),
        });
        let delete = SnapshotChange::<TestSnapshot>::delete("backup");

        assert_eq!(
            upsert,
            SnapshotChange::Upsert(Box::new(TestSnapshot {
                id: "backup".to_string(),
            }))
        );
        assert_eq!(delete, SnapshotChange::Delete("backup".to_string()));
    }
}
