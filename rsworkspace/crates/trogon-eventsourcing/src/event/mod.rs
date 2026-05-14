#[path = "event.rs"]
mod event_envelope;
mod event_headers;
mod event_headers_error;
mod header_key;
mod stream_event;

pub use event_envelope::Event;
pub use event_headers::EventHeaders;
pub use event_headers_error::EventHeadersError;
pub use header_key::HeaderKey;
pub use stream_event::StreamEvent;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventCodec, EventId, EventIdentity, EventType, StreamPosition};
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use std::str::FromStr;
    use uuid::Uuid;

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct TestEventCodec;

    impl<T> EventCodec<T> for TestEventCodec
    where
        T: Serialize + DeserializeOwned,
    {
        type Error = serde_json::Error;

        fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error> {
            serde_json::to_vec(value)
        }

        fn decode(&self, _event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<T, Self::Error> {
            serde_json::from_slice(payload)
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestEvent {
        id: String,
        value: String,
    }

    impl EventIdentity for TestEvent {}

    impl EventType for TestEvent {
        type Error = std::convert::Infallible;

        fn event_type(&self) -> Result<&'static str, Self::Error> {
            Ok("TestEvent")
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct IdentifiedEvent {
        id: EventId,
        value: String,
    }

    impl EventIdentity for IdentifiedEvent {
        fn event_id(&self) -> Option<EventId> {
            Some(self.id)
        }
    }

    impl EventType for IdentifiedEvent {
        type Error = std::convert::Infallible;

        fn event_type(&self) -> Result<&'static str, Self::Error> {
            Ok("IdentifiedEvent")
        }
    }

    #[test]
    fn event_uses_event_traits() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = Event::from_domain_event(&TestEventCodec, &payload).unwrap();

        assert_eq!(event.id.as_uuid().get_version_num(), 7);
        assert_eq!(event.r#type, "TestEvent");
        assert_eq!(
            event
                .decode_with::<TestEvent, _>("alpha", &TestEventCodec)
                .unwrap()
                .value,
            "beta"
        );
    }

    #[test]
    fn event_accepts_event_supplied_event_id() {
        let event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0001));
        let payload = IdentifiedEvent {
            id: event_id,
            value: "beta".to_string(),
        };
        let event = Event::from_domain_event(&TestEventCodec, &payload).unwrap();

        assert_eq!(event.id, event_id);
    }

    #[test]
    fn event_rejects_non_uuid_event_ids() {
        assert!(EventId::from_str("not-a-uuid").is_err());
    }

    #[test]
    fn stream_event_preserves_store_context() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = Event::from_domain_event(&TestEventCodec, &payload).unwrap();

        let recorded = event.record(
            "alpha",
            position(2),
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_position, position(2));
        assert_eq!(recorded.subject_with_prefix("events.test."), "events.test.alpha");
    }

    #[test]
    fn event_and_stream_event_decode_payloads() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = Event::from_domain_event(&TestEventCodec, &payload).unwrap();
        assert_eq!(
            event.decode_with::<TestEvent, _>("alpha", &TestEventCodec).unwrap().id,
            "alpha"
        );

        let recorded = event.record(
            "alpha",
            position(1),
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        );
        assert_eq!(
            recorded.decode_with::<TestEvent, _>(&TestEventCodec).unwrap().id,
            "alpha"
        );
    }

    #[test]
    fn event_and_stream_event_round_trip_headers() {
        let event = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let headers = EventHeaders::one(HeaderKey::new("trace-id").unwrap(), "trace-1").unwrap();

        let generated = Event::from_domain_event(&TestEventCodec, &event)
            .unwrap()
            .with_headers(headers.clone());
        assert_eq!(
            generated.decode_with::<TestEvent, _>("alpha", &TestEventCodec).unwrap(),
            event
        );
        assert_eq!(generated.headers.get("trace-id"), Some("trace-1"));

        let no_headers = Event::from_domain_event(&TestEventCodec, &event).unwrap();
        assert!(no_headers.headers.is_empty());

        let recorded = StreamEvent {
            stream_id: "alpha".to_string(),
            event: generated,
            stream_position: position(7),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_002, 0).unwrap(),
        };

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_position, position(7));
        assert_eq!(recorded.decode_with::<TestEvent, _>(&TestEventCodec).unwrap(), event);
        assert_eq!(recorded.event.headers, headers);
        assert_eq!(recorded.subject_with_prefix("events.test."), "events.test.alpha");
    }

    #[test]
    fn event_headers_validates_header_safe_names_and_values() {
        let key = HeaderKey::new("trace-id").unwrap();
        assert_eq!(key.as_str(), "trace-id");

        assert_eq!(HeaderKey::new("").unwrap_err(), EventHeadersError::EmptyName);
        assert!(matches!(
            HeaderKey::new("Nats-Expected-Last-Subject-Sequence"),
            Err(EventHeadersError::ReservedName { .. })
        ));
        assert!(matches!(
            HeaderKey::new("trace id"),
            Err(EventHeadersError::InvalidName { .. })
        ));
        assert!(matches!(
            EventHeaders::one(HeaderKey::new("trace-id").unwrap(), "line\r\nbreak"),
            Err(EventHeadersError::InvalidValue { .. })
        ));
    }
}
