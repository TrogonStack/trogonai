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

/// Decodes a stored event payload back into a domain event.
///
/// Decoders receive the event type as well as the payload so applications can
/// support aliases, migrations, or upcasters without leaking those decisions
/// into storage adapters.
pub trait EventDecode: Sized {
    /// Deserializer or migration error returned by the application-owned codec.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Deserializes the stored event payload.
    fn decode(event: EventData<'_>) -> Result<Self, Self::Error>;
}
