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
