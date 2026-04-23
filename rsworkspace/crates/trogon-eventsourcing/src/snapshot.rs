use std::borrow::Cow;

use serde::{Deserialize, Serialize};

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

pub trait SnapshotSchema {
    const SNAPSHOT_STREAM_PREFIX: &'static str;
    const CHECKPOINT_NAME: Option<&'static str> = None;

    fn snapshot_store_config() -> SnapshotStoreConfig
    where
        Self: Sized,
    {
        SnapshotStoreConfig::new(Self::SNAPSHOT_STREAM_PREFIX, Self::CHECKPOINT_NAME)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotStoreConfig {
    key_prefix: Cow<'static, str>,
    checkpoint_name: Option<Cow<'static, str>>,
}

impl SnapshotStoreConfig {
    pub fn new(key_prefix: impl Into<Cow<'static, str>>, checkpoint_name: Option<&'static str>) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_name: checkpoint_name.map(Cow::Borrowed),
        }
    }

    pub fn without_checkpoint(key_prefix: impl Into<Cow<'static, str>>) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_name: None,
        }
    }

    pub fn with_checkpoint_name(
        key_prefix: impl Into<Cow<'static, str>>,
        checkpoint_name: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_name: Some(checkpoint_name.into()),
        }
    }

    pub fn key_prefix(&self) -> &str {
        self.key_prefix.as_ref()
    }

    pub fn checkpoint_name(&self) -> Option<&str> {
        self.checkpoint_name.as_deref()
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

pub trait SnapshotRead<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn load_snapshot(
        &self,
        config: SnapshotStoreConfig,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<Option<Snapshot<SnapshotPayload>>, Self::Error>> + Send;
}

pub trait SnapshotWrite<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn save_snapshot(
        &self,
        config: SnapshotStoreConfig,
        stream_id: &StreamId,
        snapshot: Snapshot<SnapshotPayload>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;
}
