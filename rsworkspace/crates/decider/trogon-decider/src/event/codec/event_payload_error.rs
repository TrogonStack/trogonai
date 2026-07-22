/// Payload-level failure used by generated or enum-backed event codecs.
///
/// This type keeps a strict boundary between envelope routing and payload
/// validity. Unknown event types can be reported separately from payloads that
/// match a known event type but are missing the concrete case or fail source
/// deserialization.
#[derive(Debug, thiserror::Error)]
pub enum EventPayloadError<Source> {
    /// The source serializer failed after the event type was accepted.
    #[error("{0}")]
    Decode(#[source] Source),
    /// The value has no concrete event case to work with.
    ///
    /// A generated or enum-backed codec returns this when a domain event value
    /// has no concrete case set, for example an unset oneof, so
    /// [`EventEncode::encode`](crate::EventEncode::encode) cannot serialize it
    /// and [`EventType::event_type`](crate::EventType::event_type) cannot name
    /// it. The same variant also covers the symmetric decode-time failure: a
    /// stored payload whose event type matched, but whose concrete case is
    /// absent once deserialized.
    #[error("event payload is missing its concrete event case")]
    MissingEvent,
    /// The envelope event type is not recognized by this payload decoder.
    #[error("unknown event type '{event_type}'")]
    UnknownEventType {
        /// The unrecognized event type stored with the envelope.
        event_type: String,
    },
}

impl<Source> EventPayloadError<Source> {
    /// Creates an unknown event type error while preserving the stored name.
    pub fn unknown_event_type(event_type: impl Into<String>) -> Self {
        Self::UnknownEventType {
            event_type: event_type.into(),
        }
    }
}

#[cfg(test)]
mod tests;
