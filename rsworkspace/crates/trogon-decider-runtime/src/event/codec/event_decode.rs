/// Borrowed event data passed to a domain event decoder.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct EventData<'a> {
    /// Persistent event type name stored with the payload.
    pub event_type: &'a str,
    /// Serialized domain event payload.
    pub payload: &'a [u8],
}

impl<'a> EventData<'a> {
    /// Creates borrowed decoder input without copying the payload.
    pub const fn new(event_type: &'a str, payload: &'a [u8]) -> Self {
        Self { event_type, payload }
    }
}

/// Result of matching a stored event envelope against a domain event set.
///
/// Replay needs to distinguish an event this decider does not own from an event
/// it owns but cannot decode. [`Skipped`](Self::Skipped) is the non-error path
/// for the first case; the decoder's [`EventDecode::Error`] remains reserved
/// for malformed payloads, unsupported schema revisions, or other failures
/// inside the event set the decoder claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventDecodeOutcome<E> {
    /// The envelope belongs to this event set and decoded successfully.
    Decoded(E),
    /// The envelope is valid stream history, but not part of this event set.
    Skipped,
}

impl<E> EventDecodeOutcome<E> {
    /// Returns the decoded event, or `None` when the envelope was skipped.
    pub fn into_decoded(self) -> Option<E> {
        match self {
            Self::Decoded(event) => Some(event),
            Self::Skipped => None,
        }
    }

    /// Borrows the decoded event, or returns `None` when the envelope was skipped.
    pub const fn as_decoded(&self) -> Option<&E> {
        match self {
            Self::Decoded(event) => Some(event),
            Self::Skipped => None,
        }
    }
}

/// Decodes a stored event payload back into a domain event.
///
/// Decoders receive the event type as well as the payload so applications can
/// support aliases, migrations, or upcasters without leaking those decisions
/// into storage adapters. A decoder returns [`EventDecodeOutcome::Skipped`]
/// when the envelope's event type is not part of the domain event set it owns.
///
/// The runtime keeps this ownership check at the codec boundary because only
/// the application knows which event names are aliases, current variants, or
/// historical variants of a decider. Storage adapters should persist and replay
/// envelopes without embedding that domain-specific routing table.
pub trait EventDecode: Sized {
    /// Deserializer or migration error returned by the application-owned codec.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Deserializes the stored event payload when the event type is supported.
    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_new_borrows_event_type_and_payload() {
        let payload = [1, 2, 3];
        let event = EventData::new("test.event", &payload);

        assert_eq!(event.event_type, "test.event");
        assert_eq!(event.payload, payload);
    }

    #[test]
    fn decoded_outcome_exposes_event() {
        let outcome = EventDecodeOutcome::Decoded("event");

        assert_eq!(outcome.as_decoded(), Some(&"event"));
        assert_eq!(outcome.into_decoded(), Some("event"));
    }

    #[test]
    fn skipped_outcome_has_no_event() {
        let outcome = EventDecodeOutcome::<&str>::Skipped;

        assert_eq!(outcome.as_decoded(), None);
        assert_eq!(outcome.into_decoded(), None);
    }
}
