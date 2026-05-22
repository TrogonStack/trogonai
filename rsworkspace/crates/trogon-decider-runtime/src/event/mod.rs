//! Event envelope types and serialization traits.
//!
//! Domain events stay owned by application crates. This module defines the
//! storage-facing envelope and the traits adapters need to turn domain events
//! into bytes without depending on a specific serializer.

mod codec;
mod event_id;
mod event_identity;
mod event_type;
mod stream_event;

use crate::headers::Headers;

pub use codec::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError};
pub use event_id::EventId;
pub use event_identity::EventIdentity;
pub use event_type::EventType;
pub use stream_event::StreamEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Unique identity assigned to this event occurrence.
    pub id: EventId,
    /// Stable domain event name used to select the decoder.
    pub r#type: String,
    /// Serialized domain event payload.
    pub content: Vec<u8>,
    /// Metadata propagated with the event but kept outside the payload.
    pub headers: Headers,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headers::{FromEntriesError, HeaderName, HeaderNameError, HeaderValue, HeaderValueError};
    use crate::{
        EventData, EventDecode, EventDecodeOutcome, EventEncode, EventId, EventIdentity, EventType, StreamPosition,
    };
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use std::str::FromStr;
    use trogon_std::{NowV7, UuidV7Generator};
    use uuid::Uuid;

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    fn encode_json<T>(value: &T) -> Result<Vec<u8>, serde_json::Error>
    where
        T: Serialize,
    {
        serde_json::to_vec(value)
    }

    fn decode_json<T>(payload: &[u8]) -> Result<T, serde_json::Error>
    where
        T: DeserializeOwned,
    {
        serde_json::from_slice(payload)
    }

    fn encode_event<E, G>(event: &E, event_id_generator: &G, headers: &Headers) -> Event
    where
        E: EventType + EventIdentity + EventEncode,
        G: NowV7 + ?Sized,
        <E as EventType>::Error: std::fmt::Debug,
        <E as EventEncode>::Error: std::fmt::Debug,
    {
        let id = event
            .event_id()
            .unwrap_or_else(|| EventId::new(event_id_generator.now_v7()));
        Event {
            id,
            r#type: event.event_type().unwrap().to_string(),
            content: event.encode().unwrap(),
            headers: headers.clone(),
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

    impl EventEncode for TestEvent {
        type Error = serde_json::Error;

        fn encode(&self) -> Result<Vec<u8>, Self::Error> {
            encode_json(self)
        }
    }

    impl EventDecode for TestEvent {
        type Error = serde_json::Error;

        fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
            decode_json(event.payload).map(EventDecodeOutcome::Decoded)
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

    impl EventEncode for IdentifiedEvent {
        type Error = serde_json::Error;

        fn encode(&self) -> Result<Vec<u8>, Self::Error> {
            encode_json(self)
        }
    }

    #[test]
    fn event_uses_event_traits() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = encode_event(&payload, &UuidV7Generator, &Headers::empty());

        assert_eq!(event.id.as_uuid().get_version_num(), 7);
        assert_eq!(event.r#type, "TestEvent");
        assert_eq!(
            TestEvent::decode(EventData::new(&event.r#type, &event.content))
                .unwrap()
                .into_decoded()
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
        let event = encode_event(&payload, &UuidV7Generator, &Headers::empty());

        assert_eq!(event.id, event_id);
    }

    #[test]
    fn event_id_supports_uuid_debug_display_and_serde_round_trips() {
        let uuid = Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0002);
        let event_id = EventId::from(uuid);

        assert_eq!(event_id.as_uuid(), uuid);
        assert_eq!(Uuid::from(event_id), uuid);
        assert_eq!(event_id.to_string(), uuid.to_string());
        assert_eq!(format!("{event_id:?}"), format!("EventId({uuid})"));

        let encoded = serde_json::to_string(&event_id).unwrap();
        assert_eq!(serde_json::from_str::<EventId>(&encoded).unwrap(), event_id);
        assert!(serde_json::from_str::<EventId>("\"not-a-uuid\"").is_err());
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
        let event = encode_event(&payload, &UuidV7Generator, &Headers::empty());

        let recorded = StreamEvent {
            stream_id: "alpha".to_string(),
            event,
            stream_position: position(2),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        };

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_position, position(2));
    }

    #[test]
    fn event_and_stream_event_decode_payloads() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = encode_event(&payload, &UuidV7Generator, &Headers::empty());
        assert_eq!(
            TestEvent::decode(EventData::new(&event.r#type, &event.content))
                .unwrap()
                .into_decoded()
                .unwrap()
                .id,
            "alpha"
        );

        let recorded = StreamEvent {
            stream_id: "alpha".to_string(),
            event,
            stream_position: position(1),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        };
        assert_eq!(
            recorded.decode::<TestEvent>().unwrap().into_decoded().unwrap().id,
            "alpha"
        );
    }

    #[test]
    fn event_and_stream_event_round_trip_headers() {
        let event = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let headers = Headers::one(HeaderName::new("trace-id").unwrap(), "trace-1").unwrap();

        let generated = encode_event(&event, &UuidV7Generator, &headers);
        assert_eq!(
            TestEvent::decode(EventData::new(&generated.r#type, &generated.content))
                .unwrap()
                .into_decoded()
                .unwrap(),
            event
        );
        assert_eq!(
            generated.headers.get("trace-id").map(HeaderValue::as_str),
            Some("trace-1")
        );

        let no_headers = encode_event(&event, &UuidV7Generator, &Headers::empty());
        assert!(no_headers.headers.is_empty());

        let recorded = StreamEvent {
            stream_id: "alpha".to_string(),
            event: generated,
            stream_position: position(7),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_002, 0).unwrap(),
        };

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_position, position(7));
        assert_eq!(recorded.decode::<TestEvent>().unwrap().into_decoded().unwrap(), event);
        assert_eq!(recorded.event.headers, headers);
    }

    #[test]
    fn headers_accept_metadata_names_and_validate_values() {
        let name = HeaderName::new("trace-id").unwrap();
        assert_eq!(name.as_str(), "trace-id");

        let value = HeaderValue::new("trace-1").unwrap();
        assert_eq!(value.as_str(), "trace-1");
        assert_eq!(HeaderValue::from_str("trace-2").unwrap().as_str(), "trace-2");
        let typed_headers = Headers::one(HeaderName::new("typed").unwrap(), value.clone()).unwrap();
        assert_eq!(typed_headers.get("typed").map(HeaderValue::as_str), Some("trace-1"));
        assert_eq!(typed_headers.get_str("typed"), Some("trace-1"));

        assert_eq!(
            HeaderName::new("Nats-Expected-Last-Subject-Sequence").unwrap().as_str(),
            "Nats-Expected-Last-Subject-Sequence"
        );
        assert_eq!(
            HeaderName::new("Trogon-Event-Type").unwrap().as_str(),
            "Trogon-Event-Type"
        );
        assert_eq!(HeaderName::new("").unwrap_err(), HeaderNameError::Empty);
        assert_eq!(
            HeaderName::new("trace\nid").unwrap_err(),
            HeaderNameError::ContainsControlCharacter
        );
        assert_eq!(
            Headers::one(HeaderName::new("trace-id").unwrap(), "line\r\nbreak"),
            Err(HeaderValueError)
        );
        assert!(matches!(
            Headers::from_entries([("", "value")]),
            Err(FromEntriesError::InvalidName { .. })
        ));
        assert!(matches!(
            Headers::from_entries([("trace-id", "line\r\nbreak")]),
            Err(FromEntriesError::InvalidValue { .. })
        ));
        assert!(HeaderValue::new("null\0break").is_err());
    }
}
