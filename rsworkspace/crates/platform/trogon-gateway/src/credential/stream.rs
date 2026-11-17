use async_nats::jetstream::stream;
use trogon_credential_aggregate::{CREDENTIAL_EVENT_SUBJECT_PREFIX, CREDENTIAL_STREAM};

use super::event_stream_mismatch_error::CredentialEventStreamMismatchError;

pub(crate) fn stream_config() -> stream::Config {
    stream::Config {
        name: CREDENTIAL_STREAM.to_string(),
        subjects: vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")],
        allow_atomic_publish: true,
        ..Default::default()
    }
}

/// Refuses an event stream that could drop events on its own.
///
/// The credential streams are the system of record: replay rebuilds every
/// identity from the write request that opened its stream, so a stream that
/// forgets events on restart, discards them once consumed, ages them out, caps
/// them, or lets a publish erase them would lose identities that nothing else
/// records.
pub(crate) fn validate_stream_config(actual: &stream::Config) -> Result<(), CredentialEventStreamMismatchError> {
    if actual.storage != stream::StorageType::File {
        return Err(CredentialEventStreamMismatchError::NotOnDisk);
    }
    if actual.retention != stream::RetentionPolicy::Limits {
        return Err(CredentialEventStreamMismatchError::DiscardsConsumedEvents);
    }
    if !actual.max_age.is_zero() {
        return Err(CredentialEventStreamMismatchError::ExpiresByAge);
    }
    if actual.max_messages_per_subject > 0 {
        return Err(CredentialEventStreamMismatchError::CapsEventsPerSubject);
    }
    if actual.max_messages > 0 {
        return Err(CredentialEventStreamMismatchError::CapsEvents);
    }
    if actual.max_bytes > 0 {
        return Err(CredentialEventStreamMismatchError::CapsBytes);
    }
    if actual.allow_message_ttl {
        return Err(CredentialEventStreamMismatchError::ExpiresPerEvent);
    }
    if actual.allow_rollup {
        return Err(CredentialEventStreamMismatchError::RollsUpSubjects);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
