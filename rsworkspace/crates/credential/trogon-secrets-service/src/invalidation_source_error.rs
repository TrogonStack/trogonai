use trogon_credential_aggregate::{CredentialEventSubjectResolverError, CredentialProtoDecodeError};
use trogon_credential_domain::CredentialKey;
use trogon_decider_nats::StreamStoreError;
use trogonai_proto::gateway::credentials::CredentialEventPayloadError;

/// Why a credential stream message could not be read as the source of a
/// release invalidation notice.
///
/// None of these means there is nothing to publish: the message may hold the
/// event that ends a release, so whether to retry, skip, or stop is the
/// caller's decision, never a silent default.
#[derive(Debug, thiserror::Error)]
pub enum InvalidationSourceError {
    /// The message lacks what every stored credential event carries: an event
    /// id, an event type, a non-zero sequence, and a representable time.
    #[error("credential stream message is not a recorded event")]
    Unrecorded(#[source] StreamStoreError),
    /// The event type is one this build does not know, and it may be one that
    /// ends a release.
    #[error("event type '{event_type}' is not a credential event this build knows")]
    UnknownEventType { event_type: String },
    #[error("credential event payload does not decode")]
    Undecodable(#[source] CredentialEventPayloadError),
    #[error("credential event does not name a credential")]
    InvalidEvent(#[source] CredentialProtoDecodeError),
    #[error("credential owner has no subject token")]
    NoSubject(#[source] CredentialEventSubjectResolverError),
    /// The event names a credential other than the one whose subject it was
    /// appended on, so routing a notice by either would trust the other.
    #[error("event for credential '{credential}' was appended on another credential's subject '{subject}'")]
    EventFromAnotherSubject { credential: CredentialKey, subject: String },
    /// A notice's time must be after the Unix epoch, and the stream recorded
    /// this event at or before it.
    #[error("credential event at sequence {sequence} was appended no later than the Unix epoch")]
    NotAppendedAfterEpoch { sequence: u64 },
}

#[cfg(test)]
mod tests;
