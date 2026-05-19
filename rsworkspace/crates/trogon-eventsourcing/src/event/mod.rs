mod codec;
mod encode_event_error;
mod event_headers;
mod event_headers_error;
mod event_id;
mod event_identity;
mod event_type;
mod header_name;
mod stream_event;

use trogon_std::NowV7;

pub use codec::{EventData, EventDecode, EventEncode};
pub use encode_event_error::{EncodeEventError, EventEncodeError};
pub use event_headers::EventHeaders;
pub use event_headers_error::EventHeadersError;
pub use event_id::EventId;
pub use event_identity::EventIdentity;
pub use event_type::EventType;
pub use header_name::HeaderName;
pub use stream_event::StreamEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: EventId,
    pub r#type: String,
    pub content: Vec<u8>,
    pub headers: EventHeaders,
}

pub(crate) fn encode_event<E, G>(
    event: &E,
    event_id_generator: &G,
    headers: &EventHeaders,
) -> Result<Event, EventEncodeError<E>>
where
    E: EventType + EventIdentity + EventEncode,
    G: NowV7 + ?Sized,
{
    let id = event.event_id().unwrap_or_else(|| EventId::now_v7(event_id_generator));
    Ok(Event {
        id,
        r#type: event.event_type().map_err(EncodeEventError::EventType)?.to_string(),
        content: event.encode().map_err(EncodeEventError::EventEncode)?,
        headers: headers.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventData, EventDecode, EventEncode, EventId, EventIdentity, EventType, StreamPosition};
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use std::str::FromStr;
    use trogon_std::UuidV7Generator;
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

        fn decode(event: EventData<'_>) -> Result<Self, Self::Error> {
            decode_json(event.payload)
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
        let event = encode_event(&payload, &UuidV7Generator, &EventHeaders::empty()).unwrap();

        assert_eq!(event.id.as_uuid().get_version_num(), 7);
        assert_eq!(event.r#type, "TestEvent");
        assert_eq!(
            TestEvent::decode(EventData::new(&event.r#type, "alpha", &event.content))
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
        let event = encode_event(&payload, &UuidV7Generator, &EventHeaders::empty()).unwrap();

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
        let event = encode_event(&payload, &UuidV7Generator, &EventHeaders::empty()).unwrap();

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
        let event = encode_event(&payload, &UuidV7Generator, &EventHeaders::empty()).unwrap();
        assert_eq!(
            TestEvent::decode(EventData::new(&event.r#type, "alpha", &event.content))
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
        assert_eq!(recorded.decode::<TestEvent>().unwrap().id, "alpha");
    }

    #[test]
    fn event_and_stream_event_round_trip_headers() {
        let event = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let headers = EventHeaders::one(HeaderName::new("trace-id").unwrap(), "trace-1").unwrap();

        let generated = encode_event(&event, &UuidV7Generator, &headers).unwrap();
        assert_eq!(
            TestEvent::decode(EventData::new(&generated.r#type, "alpha", &generated.content)).unwrap(),
            event
        );
        assert_eq!(generated.headers.get("trace-id"), Some("trace-1"));

        let no_headers = encode_event(&event, &UuidV7Generator, &EventHeaders::empty()).unwrap();
        assert!(no_headers.headers.is_empty());

        let recorded = StreamEvent {
            stream_id: "alpha".to_string(),
            event: generated,
            stream_position: position(7),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_002, 0).unwrap(),
        };

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_position, position(7));
        assert_eq!(recorded.decode::<TestEvent>().unwrap(), event);
        assert_eq!(recorded.event.headers, headers);
    }

    #[test]
    fn event_headers_accepts_metadata_names_and_validates_values() {
        let name = HeaderName::new("trace-id").unwrap();
        assert_eq!(name.as_str(), "trace-id");

        assert_eq!(
            HeaderName::new("Nats-Expected-Last-Subject-Sequence").unwrap().as_str(),
            "Nats-Expected-Last-Subject-Sequence"
        );
        assert_eq!(
            HeaderName::new("Trogon-Event-Type").unwrap().as_str(),
            "Trogon-Event-Type"
        );
        assert_eq!(HeaderName::new("").unwrap_err(), EventHeadersError::EmptyName);
        assert!(matches!(
            EventHeaders::one(HeaderName::new("trace-id").unwrap(), "line\r\nbreak"),
            Err(EventHeadersError::InvalidValue { .. })
        ));
    }
}
