//! Concrete NATS KV access for scheduler checkpoints and failure records.
//!
//! This is the scheduler's rebuildable current-checkpoint cache, not a separate
//! read-model component. Lookups by original id, by execution subject, and by
//! listing all read over the same `v1.<uuid-simple>` key space.

use async_nats::jetstream::kv;
use bytes::Bytes;
use futures::StreamExt;
use trogon_decider_runtime::StreamPosition;
use trogon_nats::jetstream::{
    JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys,
};

use crate::processor::execution::reconciliation::{ScheduleKey, ScheduleSubject};

use super::ScheduleCheckpointRecord;
use super::codec::{
    CheckpointCodecError, decode_checkpoint_envelope, decode_checkpoint_record, encode_checkpoint_record,
};
use super::failure::{ProcessingFailureRecord, encode_failure_record};

const CHECKPOINT_KEY_PREFIX: &str = "v1.";

/// Error raised while reading or writing scheduler checkpoints.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointStoreError {
    /// Stored bytes could not be decoded; this is a data problem, not transient.
    #[error("scheduler checkpoint codec failed: {source}")]
    Codec {
        #[source]
        source: CheckpointCodecError,
        /// KV revision read with the corrupt record, when available.
        revision: Option<u64>,
        /// Stream watermark parsed from the corrupt record envelope, when available.
        watermark: Option<StreamPosition>,
        /// Last applied event id parsed from the corrupt record envelope, when available.
        last_applied_event_id: Option<String>,
    },
    /// The KV backend operation failed; callers should treat this as transient.
    #[error("scheduler checkpoint backend failed: {source}")]
    Backend {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The KV backend rejected the operation for a reason a retry cannot fix
    /// (invalid key, missing bucket stream, payload above the server limit);
    /// callers should poison rather than retry.
    #[error("scheduler checkpoint backend failed permanently: {source}")]
    PermanentBackend {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The caller passed revision 0, which is never a valid NATS KV sequence;
    /// this is a logic error, not transient.
    #[error("scheduler checkpoint revision 0 is not a valid KV revision")]
    InvalidRevision,
    /// Another writer saved this checkpoint after it was loaded (CAS revision
    /// mismatch, or the key already existed on a first write). The caller must
    /// reload and re-evaluate rather than retry the same write blindly.
    #[error("scheduler checkpoint was modified concurrently")]
    Conflict,
}

impl CheckpointStoreError {
    /// Whether the caller should retry the record rather than poison it.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Backend { .. })
    }

    fn backend(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        let source: Box<dyn std::error::Error + Send + Sync> = Box::new(source);
        if backend_error_is_permanent(source.as_ref()) {
            Self::PermanentBackend { source }
        } else {
            Self::Backend { source }
        }
    }

    /// KV revision for a corrupt checkpoint value, if the failed read had one.
    pub fn corrupt_revision(&self) -> Option<u64> {
        match self {
            Self::Codec { revision, .. } => *revision,
            Self::Backend { .. } | Self::PermanentBackend { .. } | Self::InvalidRevision | Self::Conflict => None,
        }
    }

    /// Stream watermark parsed from a corrupt checkpoint envelope, when available.
    pub fn corrupt_watermark(&self) -> Option<StreamPosition> {
        match self {
            Self::Codec { watermark, .. } => *watermark,
            Self::Backend { .. } | Self::PermanentBackend { .. } | Self::InvalidRevision | Self::Conflict => None,
        }
    }

    /// Last applied event id parsed from a corrupt checkpoint envelope, when available.
    pub fn corrupt_last_applied_event_id(&self) -> Option<&str> {
        match self {
            Self::Codec {
                last_applied_event_id, ..
            } => last_applied_event_id.as_deref(),
            Self::Backend { .. } | Self::PermanentBackend { .. } | Self::InvalidRevision | Self::Conflict => None,
        }
    }
}

/// Walks the error source chain for backend failures that retrying at the
/// ack-wait interval can never fix, so the record is durably poisoned instead
/// of redelivered forever. Everything else stays transient: misclassifying a
/// recoverable hiccup as permanent would wrongly terminate a record.
fn backend_error_is_permanent(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(err) = current {
        if let Some(entry) = err.downcast_ref::<kv::EntryError>() {
            if entry.kind() == kv::EntryErrorKind::InvalidKey {
                return true;
            }
        } else if let Some(update) = err.downcast_ref::<kv::UpdateError>() {
            if update.kind() == kv::UpdateErrorKind::InvalidKey {
                return true;
            }
        } else if let Some(create) = err.downcast_ref::<kv::CreateError>() {
            if create.kind() == kv::CreateErrorKind::InvalidKey {
                return true;
            }
        } else if let Some(publish) = err.downcast_ref::<async_nats::jetstream::context::PublishError>() {
            use async_nats::jetstream::context::PublishErrorKind;
            return matches!(
                publish.kind(),
                PublishErrorKind::StreamNotFound | PublishErrorKind::MaxPayloadExceeded
            );
        }
        current = err.source();
    }
    false
}

fn codec_load_error(value: &[u8], source: CheckpointCodecError, revision: Option<u64>) -> CheckpointStoreError {
    let envelope = decode_checkpoint_envelope(value);
    CheckpointStoreError::Codec {
        source,
        revision,
        watermark: envelope.watermark,
        last_applied_event_id: envelope.last_applied_event_id,
    }
}

/// A checkpoint record together with the KV revision it was read at, for optimistic
/// updates.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedCheckpoint {
    /// The decoded current checkpoint record.
    pub record: ScheduleCheckpointRecord,
    /// KV revision the record was read at.
    pub revision: u64,
}

/// NATS KV-backed scheduler checkpoint store.
#[derive(Debug, Clone)]
pub struct ScheduleCheckpointStore<S> {
    kv: S,
}

impl<S> ScheduleCheckpointStore<S>
where
    S: JetStreamKvEntry + JetStreamKvGet + JetStreamKvCreate + JetStreamKeyValueUpdate + JetStreamKvKeys,
{
    /// Wraps a concrete KV store handle.
    pub fn new(kv: S) -> Self {
        Self { kv }
    }

    fn checkpoint_key(key: &ScheduleKey) -> String {
        format!("{CHECKPOINT_KEY_PREFIX}{}", key.simple())
    }

    /// Reads the current checkpoint for a schedule key, returning the revision for a
    /// subsequent optimistic [`save`](Self::save). A deleted or purged entry is
    /// treated as absent.
    pub async fn load(&self, key: &ScheduleKey) -> Result<Option<LoadedCheckpoint>, CheckpointStoreError> {
        let kv_key = Self::checkpoint_key(key);
        let entry = self.kv.entry(kv_key).await.map_err(CheckpointStoreError::backend)?;

        let Some(entry) = entry else {
            return Ok(None);
        };
        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(None);
        }

        let record = decode_checkpoint_record(&entry.value)
            .map_err(|source| codec_load_error(&entry.value, source, Some(entry.revision)))?;
        Ok(Some(LoadedCheckpoint {
            record,
            revision: entry.revision,
        }))
    }

    /// Reads current checkpoint by the original schedule id.
    pub async fn load_by_id(
        &self,
        schedule_id: &crate::commands::domain::ScheduleId,
    ) -> Result<Option<LoadedCheckpoint>, CheckpointStoreError> {
        self.load(&ScheduleKey::derive(schedule_id)).await
    }

    /// Reads current checkpoint by execution subject.
    pub async fn load_by_subject(
        &self,
        subject: &ScheduleSubject,
    ) -> Result<Option<LoadedCheckpoint>, CheckpointStoreError> {
        self.load(subject.key()).await
    }

    /// Persists a checkpoint record. `revision` is `None` for a first write (create)
    /// and `Some(rev)` for an optimistic update of an existing key.
    pub async fn save(
        &self,
        record: &ScheduleCheckpointRecord,
        revision: Option<u64>,
    ) -> Result<u64, CheckpointStoreError> {
        let bytes = match encode_checkpoint_record(record) {
            Ok(bytes) => Bytes::from(bytes),
            Err(source) => {
                return Err(CheckpointStoreError::Codec {
                    source,
                    revision,
                    watermark: None,
                    last_applied_event_id: None,
                });
            }
        };
        let kv_key = Self::checkpoint_key(&record.key());

        match revision {
            Some(0) => Err(CheckpointStoreError::InvalidRevision),
            Some(revision) => match self.kv.update(&kv_key, bytes, revision).await {
                Ok(revision) => Ok(revision),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => {
                    Err(CheckpointStoreError::Conflict)
                }
                Err(error) => Err(CheckpointStoreError::backend(error)),
            },
            None => match self.kv.create(&kv_key, bytes).await {
                Ok(revision) => Ok(revision),
                Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Err(CheckpointStoreError::Conflict),
                Err(error) => Err(CheckpointStoreError::backend(error)),
            },
        }
    }

    /// Enumerates every stored schedule checkpoint record. Failure and non-checkpoint
    /// keys are skipped.
    pub async fn list(&self) -> Result<Vec<ScheduleCheckpointRecord>, CheckpointStoreError> {
        let mut keys = self.kv.keys().await.map_err(CheckpointStoreError::backend)?;
        let mut records = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(CheckpointStoreError::backend)?;
            if !key.starts_with(CHECKPOINT_KEY_PREFIX) {
                continue;
            }
            if let Some(value) = self.kv.get(key).await.map_err(CheckpointStoreError::backend)? {
                match decode_checkpoint_record(&value) {
                    Ok(record) => records.push(record),
                    Err(error) => {
                        tracing::warn!(error = %error, "skipping corrupt scheduler checkpoint during enumeration");
                    }
                }
            }
        }
        Ok(records)
    }

    /// Durably records a processing failure. An already-present failure record
    /// for the same coordinates is treated as success so redelivery converges.
    pub async fn record_failure(&self, failure: &ProcessingFailureRecord) -> Result<(), CheckpointStoreError> {
        let bytes = Bytes::from(
            encode_failure_record(failure).map_err(|source| CheckpointStoreError::Codec {
                source: CheckpointCodecError::Json { source },
                revision: None,
                watermark: None,
                last_applied_event_id: None,
            })?,
        );

        match self.kv.create(&failure.key(), bytes).await {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(CheckpointStoreError::backend(error)),
        }
    }
}

#[cfg(test)]
mod tests;
