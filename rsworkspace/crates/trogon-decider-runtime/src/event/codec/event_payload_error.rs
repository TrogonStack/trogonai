/// Payload-level failure used by generated or enum-backed event decoders.
///
/// This type keeps a strict boundary between envelope routing and payload
/// validity. Unknown event types can be reported separately from payloads that
/// match a known event type but are missing the concrete case or fail source
/// deserialization.
#[derive(Debug)]
pub enum EventPayloadError<Source> {
    /// The source serializer failed after the event type was accepted.
    Decode(Source),
    /// The decoded payload did not contain the concrete event case.
    MissingEvent,
    /// The envelope event type is not recognized by this payload decoder.
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

impl<Source> std::fmt::Display for EventPayloadError<Source>
where
    Source: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(source) => write!(f, "{source}"),
            Self::MissingEvent => f.write_str("event payload is missing its concrete event case"),
            Self::UnknownEventType { event_type } => write!(f, "unknown event type '{event_type}'"),
        }
    }
}

impl<Source> std::error::Error for EventPayloadError<Source>
where
    Source: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(source) => Some(source),
            Self::MissingEvent | Self::UnknownEventType { .. } => None,
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
        let error = EventPayloadError::<std::io::Error>::unknown_event_type("trogon.cron.jobs.v1.Unknown");

        assert!(matches!(
            &error,
            EventPayloadError::UnknownEventType { event_type }
                if event_type == "trogon.cron.jobs.v1.Unknown"
        ));
        assert_eq!(error.to_string(), "unknown event type 'trogon.cron.jobs.v1.Unknown'");
        assert!(error.source().is_none());
    }
}
