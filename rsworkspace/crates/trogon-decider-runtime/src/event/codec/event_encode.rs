/// Encodes a domain event into the bytes stored in an [`Event`](crate::Event).
///
/// The event type and headers live in the envelope, not inside this trait. This
/// keeps payload serialization independent from storage metadata.
pub trait EventEncode {
    /// Serializer error returned by the application-owned codec.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serializes the domain event payload.
    fn encode(&self) -> Result<Vec<u8>, Self::Error>;
}
