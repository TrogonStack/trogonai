use super::constants::CREDENTIAL_STREAM;

/// A setting on the credential event stream that could drop events.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CredentialEventStreamMismatchError {
    #[error("stream '{CREDENTIAL_STREAM}' must keep events on disk")]
    NotOnDisk,
    #[error("stream '{CREDENTIAL_STREAM}' must keep events after they are consumed")]
    DiscardsConsumedEvents,
    #[error("stream '{CREDENTIAL_STREAM}' must not expire events by age")]
    ExpiresByAge,
    #[error("stream '{CREDENTIAL_STREAM}' must not cap the events kept per subject")]
    CapsEventsPerSubject,
    #[error("stream '{CREDENTIAL_STREAM}' must not cap the events it keeps")]
    CapsEvents,
    #[error("stream '{CREDENTIAL_STREAM}' must not cap the bytes it keeps")]
    CapsBytes,
    #[error("stream '{CREDENTIAL_STREAM}' must not let an event set its own expiry")]
    ExpiresPerEvent,
    #[error("stream '{CREDENTIAL_STREAM}' must not let a publish roll up a subject")]
    RollsUpSubjects,
}
