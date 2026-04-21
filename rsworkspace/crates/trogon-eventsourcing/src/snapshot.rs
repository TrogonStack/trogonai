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
        match Self::CHECKPOINT_NAME {
            Some(checkpoint_name) => SnapshotStoreConfig::with_checkpoint(
                Self::SNAPSHOT_STREAM_PREFIX,
                format!(
                    "_snapshot.{}.{}",
                    Self::SNAPSHOT_STREAM_PREFIX.trim_end_matches('.'),
                    checkpoint_name
                ),
            ),
            None => SnapshotStoreConfig::without_checkpoint(Self::SNAPSHOT_STREAM_PREFIX),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotStoreConfig {
    key_prefix: Cow<'static, str>,
    checkpoint_key: Option<Cow<'static, str>>,
}

impl SnapshotStoreConfig {
    pub fn new(
        key_prefix: impl Into<Cow<'static, str>>,
        checkpoint_key: Option<&'static str>,
    ) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_key: checkpoint_key.map(Cow::Borrowed),
        }
    }

    pub fn without_checkpoint(key_prefix: impl Into<Cow<'static, str>>) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_key: None,
        }
    }

    pub fn with_checkpoint(
        key_prefix: impl Into<Cow<'static, str>>,
        checkpoint_key: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            key_prefix: key_prefix.into(),
            checkpoint_key: Some(checkpoint_key.into()),
        }
    }

    pub fn key_prefix(&self) -> &str {
        self.key_prefix.as_ref()
    }

    pub fn checkpoint_key(&self) -> Option<&str> {
        self.checkpoint_key.as_deref()
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
