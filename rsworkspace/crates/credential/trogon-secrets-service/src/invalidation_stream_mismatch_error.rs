use super::constants::{
    INVALIDATION_MAX_MESSAGE_SIZE, INVALIDATION_REPLAY_WINDOW, INVALIDATION_STREAM, INVALIDATION_SUBJECTS,
};

/// An existing invalidation stream that would weaken a guarantee holders rely
/// on.
///
/// Each variant names the rule, never a stored notice; the module docs of
/// [`InvalidationStream`](super::InvalidationStream) say why each rule exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidationStreamMismatchError {
    #[error("stream '{INVALIDATION_STREAM}' must use file storage")]
    Storage,
    #[error("stream '{INVALIDATION_STREAM}' must use limits retention")]
    Retention,
    #[error("stream '{INVALIDATION_STREAM}' must deny deleting a notice")]
    DenyDelete,
    #[error("stream '{INVALIDATION_STREAM}' must deny purging notices")]
    DenyPurge,
    #[error("stream '{INVALIDATION_STREAM}' must not let a publish roll up and remove stored notices")]
    Rollup,
    #[error("stream '{INVALIDATION_STREAM}' must acknowledge publishes")]
    NoAck,
    #[error("stream '{INVALIDATION_STREAM}' must not be sealed, since a sealed stream stores no new notice")]
    Sealed,
    #[error("stream '{INVALIDATION_STREAM}' must store a notice before acknowledging it")]
    AsyncPersist,
    #[error(
        "stream '{INVALIDATION_STREAM}' must keep notices for the {} second replay window or longer",
        INVALIDATION_REPLAY_WINDOW.as_secs()
    )]
    MaxAge,
    #[error("stream '{INVALIDATION_STREAM}' must not let a notice set its own expiry")]
    MessageTtl,
    #[error("stream '{INVALIDATION_STREAM}' must not be a counter stream, since a counter stream refuses every notice")]
    MessageCounter,
    #[error("stream '{INVALIDATION_STREAM}' must not limit the notices kept per subject")]
    MaxMessagesPerSubject,
    #[error("stream '{INVALIDATION_STREAM}' must take messages of {INVALIDATION_MAX_MESSAGE_SIZE} bytes or more")]
    MaxMessageSize,
    #[error("stream '{INVALIDATION_STREAM}' must keep a duplicate window of two minutes or longer")]
    DuplicateWindow,
    #[error("stream '{INVALIDATION_STREAM}' must not transform the subjects notices are stored on")]
    SubjectTransform,
    #[error("stream '{INVALIDATION_STREAM}' must keep at least the replicas the deployment asked for")]
    Replicas,
    #[error(
        "stream '{INVALIDATION_STREAM}' is missing the subject notices are published on: '{INVALIDATION_SUBJECTS}'"
    )]
    MissingSubject,
}

#[cfg(test)]
mod tests;
