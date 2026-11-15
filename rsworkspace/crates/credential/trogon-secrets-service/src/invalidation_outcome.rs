use std::time::SystemTime;

use async_nats::jetstream::message::StreamMessage;
use trogon_credential_aggregate::proto::event_key;
use trogon_credential_aggregate::{credential_event_subject, invalidation_reason};
use trogon_credential_domain::CredentialKey;
use trogon_decider_nats::record_stream_message;
use trogon_decider_runtime::EventDecodeOutcome;
use trogonai_proto::gateway::credentials::{CredentialEventPayloadError, v1};

use super::{InvalidationNotice, InvalidationSourceError, OwnerTenants};

/// What one message of the credential stream asks of the invalidation
/// publisher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvalidationOutcome {
    /// The event changed what the credential releases, so its holders must
    /// drop their copies.
    Publish(InvalidationNotice),
    /// The event leaves what the credential releases as it was.
    NothingToPublish,
    /// The event changed what the credential releases, but no tenant holds
    /// its owner, so no account could import a notice. Under the tenant
    /// owners the service runs with, no tenant is released the credential
    /// either, so only a copy released under an earlier file could be stale,
    /// and it expires within the ADR#0049 bound.
    OwnerHeldByNoTenant { credential: CredentialKey },
}

impl InvalidationOutcome {
    /// Reads `message`, as the credential stream stored it, and decides
    /// whether it calls for a notice and to which tenant, asking `owners`.
    ///
    /// The notice is timed when the stream appended the event and carries its
    /// sequence, so a redelivered message builds the same notice. An event
    /// that changes nothing a release hands out is never checked further: it
    /// publishes nothing whatever it names.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidationSourceError`] when the message is not a recorded
    /// credential event this build can read, when an event that ends a release
    /// names no credential or another credential than its subject's, or when
    /// it was appended no later than the Unix epoch.
    pub fn of(message: StreamMessage, owners: &impl OwnerTenants) -> Result<Self, InvalidationSourceError> {
        let subject = message.subject.to_string();
        let event = record_stream_message(message, subject.as_str()).map_err(InvalidationSourceError::Unrecorded)?;
        let decoded = match event
            .decode::<v1::CredentialEvent>()
            .map_err(InvalidationSourceError::Undecodable)?
        {
            EventDecodeOutcome::Decoded(decoded) => decoded,
            EventDecodeOutcome::Skipped => {
                return Err(InvalidationSourceError::UnknownEventType {
                    event_type: event.event.r#type,
                });
            }
        };
        let case = decoded.event.ok_or(InvalidationSourceError::Undecodable(
            CredentialEventPayloadError::MissingEvent,
        ))?;
        let Some(reason) = invalidation_reason(&case) else {
            return Ok(Self::NothingToPublish);
        };

        let credential = event_key(&case).map_err(InvalidationSourceError::InvalidEvent)?;
        let own_subject = credential_event_subject(&credential).map_err(InvalidationSourceError::NoSubject)?;
        if own_subject.as_str() != subject {
            return Err(InvalidationSourceError::EventFromAnotherSubject { credential, subject });
        }
        let Some(tenant) = owners.holder(credential.owner_id()) else {
            return Ok(Self::OwnerHeldByNoTenant { credential });
        };

        InvalidationNotice::new(
            &tenant,
            &credential,
            reason,
            SystemTime::from(event.recorded_at),
            event.stream_position,
        )
        .map(Self::Publish)
    }
}

#[cfg(test)]
mod tests;
