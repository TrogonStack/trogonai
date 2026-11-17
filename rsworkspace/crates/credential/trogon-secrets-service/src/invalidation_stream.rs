//! The JetStream stream that holds release invalidation notices, the
//! configuration it is created with, and the check that an existing stream
//! keeps it.
//!
//! [`INVALIDATION_STREAM`] takes every tenant's notices on
//! [`INVALIDATION_SUBJECTS`]. The fourth token of a notice subject is the
//! account of the tenant holding the credential's owner, so an account imports
//! only its own (ADR#0081 Decision 3 amendment). A notice carries metadata,
//! never a value (ADR#0023 Decision 3).
//!
//! # What an existing stream must keep
//!
//! A holder drops its copies of a credential when a notice for it arrives, and
//! a replica whose consumer was down resumes [`INVALIDATION_REPLAY_WINDOW`]
//! back, the longest a released copy can still be held. Every replica reads
//! through its own consumer (ADR#0023 Decision 6), so each one depends on the
//! stream keeping every notice the publisher was told it stored, for at least
//! that window. The check refuses a stream configured to:
//!
//! - store a notice without acknowledging it (`no_ack`), acknowledge one before
//!   it is on disk (`persist_mode: async`), or take no new notice at all
//!   (`sealed`), so the publisher could not settle the credential event behind
//!   a notice on the strength of its acknowledgement;
//! - drop notices before the replay window ends: a `max_age` shorter than it, a
//!   per-message TTL, a per-subject limit, or a retention other than limits,
//!   which removes a notice once the consumers present have read it, before a
//!   resuming replica can;
//! - lose them on a restart (anything but file storage) or with a node (fewer
//!   replicas than the deployment asked for);
//! - let an operator or a compromised client remove or replace them
//!   (`deny_delete` and `deny_purge` off, or `allow_rollup` on, which lets any
//!   publisher replace every stored notice on a subject with one header);
//! - store a redelivered credential event's notice twice (a duplicate window
//!   shorter than two minutes, which the publisher's redelivery must fit in);
//! - refuse a notice for its size (a maximum message size under the 1 KiB the
//!   largest notice fits in with its headers);
//! - refuse every notice as a counter stream (`allow_msg_counter`), which
//!   takes only publishes that add to a counter;
//! - store notices under another subject (`subject_transform`), so a holder
//!   filtering on the subject a notice was published to would never read it;
//! - leave out the subject, so a notice would be published to nothing.
//!
//! What is left alone is capacity and placement: the discard policy,
//! `max_bytes`, `max_msgs`, compression, placement, direct get and extra
//! subjects. The stream is created with no byte or count bound, so neither
//! discard policy ever drops a notice; it is created with `discard: old` so
//! that, should an operator add a bound, a full stream sheds its oldest notice,
//! the one most likely past the replay window, rather than refusing the newest.
//! Either way a notice that never arrives costs a holder only until its copy's
//! `valid_until`, the ADR#0049 bound. A longer or unlimited `max_age` or
//! duplicate window, a larger or unlimited message size, and more replicas than
//! asked for are stronger, not weaker.

use std::num::NonZeroUsize;

use async_nats::jetstream::stream::{Config, DiscardPolicy, PersistenceMode, RetentionPolicy, StorageType};

use super::InvalidationStreamMismatchError;
use super::constants::{
    INVALIDATION_DUPLICATE_WINDOW, INVALIDATION_MAX_AGE, INVALIDATION_MAX_MESSAGE_SIZE, INVALIDATION_REPLAY_WINDOW,
    INVALIDATION_STREAM, INVALIDATION_SUBJECTS,
};

/// The stream that holds release invalidation notices.
pub struct InvalidationStream;

impl InvalidationStream {
    /// The configuration the stream is created with.
    ///
    /// `replicas` follows the cluster the deployment runs on, three in
    /// production and one on a single node, so it is the caller's to state.
    /// Direct get is on so a holder can read the latest notice of one
    /// credential from any replica without a consumer.
    pub fn config(replicas: NonZeroUsize) -> Config {
        Config {
            name: INVALIDATION_STREAM.to_owned(),
            subjects: vec![INVALIDATION_SUBJECTS.to_owned()],
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            discard: DiscardPolicy::Old,
            max_age: INVALIDATION_MAX_AGE,
            duplicate_window: INVALIDATION_DUPLICATE_WINDOW,
            max_message_size: INVALIDATION_MAX_MESSAGE_SIZE,
            deny_delete: true,
            deny_purge: true,
            allow_direct: true,
            num_replicas: replicas.get(),
            ..Default::default()
        }
    }

    /// Checks that `actual` keeps every guarantee in the module docs, naming
    /// the first field that does not.
    ///
    /// # Errors
    ///
    /// Returns the [`InvalidationStreamMismatchError`] of the first rule
    /// `actual` breaks.
    pub fn validate(replicas: NonZeroUsize, actual: &Config) -> Result<(), InvalidationStreamMismatchError> {
        if actual.storage != StorageType::File {
            return Err(InvalidationStreamMismatchError::Storage);
        }
        if actual.retention != RetentionPolicy::Limits {
            return Err(InvalidationStreamMismatchError::Retention);
        }
        if !actual.deny_delete {
            return Err(InvalidationStreamMismatchError::DenyDelete);
        }
        if !actual.deny_purge {
            return Err(InvalidationStreamMismatchError::DenyPurge);
        }
        if actual.allow_rollup {
            return Err(InvalidationStreamMismatchError::Rollup);
        }
        if actual.no_ack {
            return Err(InvalidationStreamMismatchError::NoAck);
        }
        if actual.sealed {
            return Err(InvalidationStreamMismatchError::Sealed);
        }
        if actual.persist_mode == Some(PersistenceMode::Async) {
            return Err(InvalidationStreamMismatchError::AsyncPersist);
        }
        if !actual.max_age.is_zero() && actual.max_age < INVALIDATION_REPLAY_WINDOW {
            return Err(InvalidationStreamMismatchError::MaxAge);
        }
        if actual.allow_message_ttl {
            return Err(InvalidationStreamMismatchError::MessageTtl);
        }
        if actual.allow_message_counter {
            return Err(InvalidationStreamMismatchError::MessageCounter);
        }
        if actual.max_messages_per_subject > 0 {
            return Err(InvalidationStreamMismatchError::MaxMessagesPerSubject);
        }
        if actual.max_message_size > 0 && actual.max_message_size < INVALIDATION_MAX_MESSAGE_SIZE {
            return Err(InvalidationStreamMismatchError::MaxMessageSize);
        }
        if actual.duplicate_window < INVALIDATION_DUPLICATE_WINDOW {
            return Err(InvalidationStreamMismatchError::DuplicateWindow);
        }
        if actual.subject_transform.is_some() {
            return Err(InvalidationStreamMismatchError::SubjectTransform);
        }
        if actual.num_replicas < replicas.get() {
            return Err(InvalidationStreamMismatchError::Replicas);
        }
        if !actual.subjects.iter().any(|subject| subject == INVALIDATION_SUBJECTS) {
            return Err(InvalidationStreamMismatchError::MissingSubject);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
