/// Payload-level failure used by generated or enum-backed event decoders.
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
    /// The decoded payload did not contain the concrete event case.
    #[error("event payload is missing its concrete event case")]
    MissingEvent,
    /// The envelope event type is not recognized by this payload decoder.
    #[error("unknown event type '{event_type}'")]
    UnknownEventType { event_type: String },
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
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn decode_preserves_source_error() {
        let error = EventPayloadError::Decode(std::io::Error::other("decode failed"));

        assert_eq!(error.to_string(), "decode failed");
        assert!(error.source().is_some());
    }

    #[test]
    fn missing_event_has_no_source() {
        let error = EventPayloadError::<std::io::Error>::MissingEvent;

        assert_eq!(error.to_string(), "event payload is missing its concrete event case");
        assert!(error.source().is_none());
    }

    #[test]
    fn unknown_event_type_captures_event_type() {
        let error = EventPayloadError::<std::io::Error>::unknown_event_type("trogonai.scheduler.schedules.v1.Unknown");

        assert!(matches!(
            &error,
            EventPayloadError::UnknownEventType { event_type }
                if event_type == "trogonai.scheduler.schedules.v1.Unknown"
        ));
        assert_eq!(
            error.to_string(),
            "unknown event type 'trogonai.scheduler.schedules.v1.Unknown'"
        );
        assert!(error.source().is_none());
    }
}
