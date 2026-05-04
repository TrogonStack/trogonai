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

    fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> impl std::future::Future<Output = Result<ReadSnapshotResponse<SnapshotPayload>, Self::Error>> + Send;
}

pub trait SnapshotWrite<SnapshotPayload, StreamId: ?Sized>: Send + Sync {
    type Error;

    fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, SnapshotPayload, StreamId>,
    ) -> impl std::future::Future<Output = Result<WriteSnapshotResponse, Self::Error>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSnapshotRequest<'a, StreamId: ?Sized> {
    pub config: SnapshotStoreConfig,
    pub stream_id: &'a StreamId,
}

impl<'a, StreamId: ?Sized> ReadSnapshotRequest<'a, StreamId> {
    pub const fn new(config: SnapshotStoreConfig, stream_id: &'a StreamId) -> Self {
        Self { config, stream_id }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadSnapshotResponse<SnapshotPayload> {
    pub snapshot: Option<Snapshot<SnapshotPayload>>,
}

impl<SnapshotPayload> ReadSnapshotResponse<SnapshotPayload> {
    pub const fn new(snapshot: Option<Snapshot<SnapshotPayload>>) -> Self {
        Self { snapshot }
    }

    pub fn into_snapshot(self) -> Option<Snapshot<SnapshotPayload>> {
        self.snapshot
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WriteSnapshotRequest<'a, SnapshotPayload, StreamId: ?Sized> {
    pub config: SnapshotStoreConfig,
    pub stream_id: &'a StreamId,
    pub snapshot: Snapshot<SnapshotPayload>,
}

impl<'a, SnapshotPayload, StreamId: ?Sized> WriteSnapshotRequest<'a, SnapshotPayload, StreamId> {
    pub const fn new(
        config: SnapshotStoreConfig,
        stream_id: &'a StreamId,
        snapshot: Snapshot<SnapshotPayload>,
    ) -> Self {
        Self {
            config,
            stream_id,
            snapshot,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteSnapshotResponse;

impl WriteSnapshotResponse {
    pub const fn new() -> Self {
        Self
    }
}
