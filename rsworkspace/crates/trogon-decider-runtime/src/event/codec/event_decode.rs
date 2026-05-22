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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventDecodeOutcome<E> {
    /// The envelope belongs to this event set and decoded successfully.
    Decoded(E),
    /// The envelope is not part of this event set.
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
pub trait EventDecode: Sized {
    /// Deserializer or migration error returned by the application-owned codec.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Deserializes the stored event payload when the event type is supported.
    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error>;
}
