//! [`ReplayStore`] over the shared `AAUTH_REPLAY` NATS KV bucket.
//!
//! Every edge instance records into the same bucket, so a record accepted by
//! one instance is refused by all of them. The check and the record are one
//! KV `create`, which JetStream accepts only while the key has no live
//! revision, so two concurrent duplicates can never both be admitted.
//!
//! The bucket is provisioned out of band and only opened here: the edge holds
//! grants to read and create keys, never to create or edit streams.

use std::fmt;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::context::KeyValueError;
use async_nats::jetstream::kv::{CreateErrorKind, StatusError, Store};
use async_trait::async_trait;
use bytes::Bytes;

use crate::constants::{AAUTH_REPLAY_BUCKET, AAUTH_REPLAY_HISTORY, REPLAY_RECORD_LIFETIME_SECS};
use crate::replay::{ReplayError, ReplayStore};

/// The history and maximum age the replay bucket has to keep for its records
/// to outlive every signature they guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayBucketSpec {
    history: i64,
    max_age: Duration,
}

impl ReplayBucketSpec {
    pub const REQUIRED: Self = Self {
        history: AAUTH_REPLAY_HISTORY,
        max_age: Duration::from_secs(REPLAY_RECORD_LIFETIME_SECS as u64),
    };

    #[must_use]
    pub fn history(&self) -> i64 {
        self.history
    }

    #[must_use]
    pub fn max_age(&self) -> Duration {
        self.max_age
    }
}

impl fmt::Display for ReplayBucketSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "history {} and max age {}s", self.history, self.max_age.as_secs())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayBucketError {
    #[error("replay bucket {bucket} could not be opened; it has to be provisioned before the edge starts", bucket = AAUTH_REPLAY_BUCKET)]
    Open(#[source] KeyValueError),
    #[error("replay bucket {bucket} configuration could not be read", bucket = AAUTH_REPLAY_BUCKET)]
    Inspect(#[source] StatusError),
    #[error(transparent)]
    Incompatible(#[from] IncompatibleReplayBucketError),
}

#[derive(Debug, thiserror::Error)]
#[error("replay bucket {bucket} keeps {actual}, but the edge requires {expected}", bucket = AAUTH_REPLAY_BUCKET)]
pub struct IncompatibleReplayBucketError {
    expected: ReplayBucketSpec,
    actual: ReplayBucketSpec,
}

impl IncompatibleReplayBucketError {
    #[must_use]
    pub fn expected(&self) -> ReplayBucketSpec {
        self.expected
    }

    #[must_use]
    pub fn actual(&self) -> ReplayBucketSpec {
        self.actual
    }
}

/// A record asked to live longer than the bucket keeps any key, which would
/// let the guarded request replay once the bucket drops the record.
#[derive(Debug, thiserror::Error)]
#[error("a replay record of {ttl_secs}s outlives the {max_age_secs}s maximum age of replay bucket {bucket}", max_age_secs = self.max_age.as_secs(), bucket = AAUTH_REPLAY_BUCKET)]
pub struct RecordOutlivesBucketError {
    ttl_secs: u32,
    max_age: Duration,
}

pub struct NatsKvReplayStore {
    store: Store,
    spec: ReplayBucketSpec,
}

impl NatsKvReplayStore {
    /// Opens the provisioned bucket and refuses it unless it keeps exactly
    /// [`ReplayBucketSpec::REQUIRED`]. Never creates the bucket.
    ///
    /// # Errors
    ///
    /// [`ReplayBucketError::Open`] when the bucket does not exist or cannot be
    /// reached, [`ReplayBucketError::Inspect`] when its configuration cannot be
    /// read, and [`ReplayBucketError::Incompatible`] when it keeps a different
    /// history or maximum age.
    pub async fn open(jetstream: &jetstream::Context) -> Result<Self, ReplayBucketError> {
        let store = jetstream
            .get_key_value(AAUTH_REPLAY_BUCKET)
            .await
            .map_err(ReplayBucketError::Open)?;
        let status = store.status().await.map_err(ReplayBucketError::Inspect)?;
        let actual = ReplayBucketSpec {
            history: status.history(),
            max_age: status.max_age(),
        };
        let expected = ReplayBucketSpec::REQUIRED;
        if actual != expected {
            return Err(IncompatibleReplayBucketError { expected, actual }.into());
        }
        Ok(Self { store, spec: actual })
    }
}

#[async_trait]
impl ReplayStore for NatsKvReplayStore {
    async fn check_and_insert(&self, key: &str, ttl_secs: u32) -> Result<bool, ReplayError> {
        if Duration::from_secs(u64::from(ttl_secs)) > self.spec.max_age {
            return Err(ReplayError::Backend(Box::new(RecordOutlivesBucketError {
                ttl_secs,
                max_age: self.spec.max_age,
            })));
        }
        match self.store.create(key, Bytes::new()).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == CreateErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(ReplayError::Backend(Box::new(err))),
        }
    }
}

#[cfg(test)]
mod tests;
