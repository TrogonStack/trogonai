//! The two JetStream streams that hold audit records (ADR#0086 Decision 2), the
//! configuration each must have, and the check that an existing stream has it.
//!
//! `AUDIT` takes the decision, credential, change and integrity kinds, and
//! `AUDIT_CHECKPOINT` takes the checkpoint kind. Each stream's subjects are the
//! kinds' subject families (`platform.v1.audit.{kind}.>`), built from the same
//! constants [`AuditSubject`](crate::AuditSubject) renders with, and the tests
//! hold every subject it can render to a stream.
//!
//! # What an existing stream must keep
//!
//! The writer refuses an action it cannot record, and an operator reads the
//! stream as the truth about what happened. Both depend on the stream never
//! losing or refusing to keep a record it acknowledged, so the check refuses a
//! stream configured to:
//!
//! - drop records on a clock or a count: a `max_age`, a per-message TTL, a
//!   per-subject limit, a retention other than limits, or `discard: old`, which
//!   makes a full stream evict the oldest record where `discard: new` refuses
//!   the write;
//! - lose them on a restart (anything but file storage);
//! - publish without acknowledging (`no_ack`), so the writer could never learn
//!   a record was kept and would time out on every one;
//! - let an operator or a compromised client remove them (`deny_delete` and
//!   `deny_purge` off);
//! - lose them with a node (fewer replicas than the deployment asked for);
//! - store a retried record twice (a duplicate window shorter than the sink's
//!   retries span, [`STREAM_DUPLICATE_WINDOW`]);
//! - leave out a subject, so a record would be published to nothing.
//!
//! What is left alone is capacity and placement: `max_bytes`, `max_msgs`, the
//! maximum message size, compression, placement and extra subjects. They bound
//! how much the stream takes, and with `discard: new` a bound turns into a
//! refusal that the writer already reports as `stream_full`, never into a lost
//! record. A longer duplicate window or more replicas than asked for is
//! likewise stronger, not weaker.

use std::num::NonZeroUsize;
use std::time::Duration;

use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};

use crate::audit_stream_mismatch_error::AuditStreamMismatchError;
use crate::constants::{
    STREAM_AUDIT, STREAM_AUDIT_CHECKPOINT, STREAM_DUPLICATE_WINDOW, SUBJECT_BINDING_VERSION, SUBJECT_KIND_CHANGE,
    SUBJECT_KIND_CHECKPOINT, SUBJECT_KIND_CREDENTIAL, SUBJECT_KIND_DECISION, SUBJECT_KIND_INTEGRITY, SUBJECT_PREFIX,
    SUBJECT_ROOT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditStream {
    /// `AUDIT`: decision, credential, change and integrity records.
    Audit,
    /// `AUDIT_CHECKPOINT`: signed checkpoints, apart so the volume of records
    /// a checkpoint vouches for can never fill the stream that holds it.
    Checkpoint,
}

impl AuditStream {
    pub const ALL: [Self; 2] = [Self::Audit, Self::Checkpoint];

    pub fn name(self) -> &'static str {
        match self {
            Self::Audit => STREAM_AUDIT,
            Self::Checkpoint => STREAM_AUDIT_CHECKPOINT,
        }
    }

    fn kinds(self) -> &'static [&'static str] {
        match self {
            Self::Audit => &[
                SUBJECT_KIND_DECISION,
                SUBJECT_KIND_CREDENTIAL,
                SUBJECT_KIND_CHANGE,
                SUBJECT_KIND_INTEGRITY,
            ],
            Self::Checkpoint => &[SUBJECT_KIND_CHECKPOINT],
        }
    }

    pub fn subjects(self) -> Vec<String> {
        self.kinds()
            .iter()
            .map(|kind| format!("{SUBJECT_PREFIX}.{SUBJECT_BINDING_VERSION}.{SUBJECT_ROOT}.{kind}.>"))
            .collect()
    }

    /// The configuration the stream is created with.
    ///
    /// `replicas` is three in production and one in the dev compose, so it is
    /// the caller's to state.
    pub fn config(self, replicas: NonZeroUsize) -> Config {
        Config {
            name: self.name().to_owned(),
            subjects: self.subjects(),
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            discard: DiscardPolicy::New,
            deny_delete: true,
            deny_purge: true,
            duplicate_window: STREAM_DUPLICATE_WINDOW,
            max_age: Duration::ZERO,
            num_replicas: replicas.get(),
            ..Default::default()
        }
    }

    /// Checks that `actual` keeps every guarantee in the module docs, naming the
    /// first field that does not.
    pub(crate) fn validate(self, replicas: NonZeroUsize, actual: &Config) -> Result<(), AuditStreamMismatchError> {
        let stream = self.name();
        if actual.storage != StorageType::File {
            return Err(AuditStreamMismatchError::Storage { stream });
        }
        if actual.retention != RetentionPolicy::Limits {
            return Err(AuditStreamMismatchError::Retention { stream });
        }
        if actual.discard != DiscardPolicy::New {
            return Err(AuditStreamMismatchError::Discard { stream });
        }
        if !actual.deny_delete {
            return Err(AuditStreamMismatchError::DenyDelete { stream });
        }
        if !actual.deny_purge {
            return Err(AuditStreamMismatchError::DenyPurge { stream });
        }
        if actual.no_ack {
            return Err(AuditStreamMismatchError::NoAck { stream });
        }
        if !actual.max_age.is_zero() {
            return Err(AuditStreamMismatchError::MaxAge { stream });
        }
        if actual.allow_message_ttl {
            return Err(AuditStreamMismatchError::MessageTtl { stream });
        }
        if actual.max_messages_per_subject > 0 {
            return Err(AuditStreamMismatchError::MaxMessagesPerSubject { stream });
        }
        if actual.duplicate_window < STREAM_DUPLICATE_WINDOW {
            return Err(AuditStreamMismatchError::DuplicateWindow { stream });
        }
        if actual.num_replicas < replicas.get() {
            return Err(AuditStreamMismatchError::Replicas { stream });
        }
        if let Some(subject) = self.subjects().into_iter().find(|s| !actual.subjects.contains(s)) {
            return Err(AuditStreamMismatchError::MissingSubject { stream, subject });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
