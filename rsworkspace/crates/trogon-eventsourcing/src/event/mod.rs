mod event_headers;
mod event_headers_error;
mod header_key;
mod stream_event;

use trogon_std::UuidV7Generator;

use crate::{
    EncodeEventError, EventDecode, EventEncode, EventEncodeError, EventId, EventIdentity, EventType, StreamPosition,
};

pub use event_headers::EventHeaders;
pub use event_headers_error::EventHeadersError;
pub use header_key::HeaderKey;
pub use stream_event::StreamEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: EventId,
    pub r#type: String,
    pub content: Vec<u8>,
    pub headers: EventHeaders,
}

impl Event {
    pub fn from_domain_event<E>(event: &E) -> Result<Self, EventEncodeError<E>>
    where
        E: EventType + EventIdentity + EventEncode,
    {
        let id = event.event_id().unwrap_or_else(|| EventId::now_v7(&UuidV7Generator));
        Ok(Self {
            id,
            r#type: event.event_type().map_err(EncodeEventError::EventType)?.to_string(),
            content: event.encode().map_err(EncodeEventError::EventEncode)?,
            headers: EventHeaders::empty(),
        })
    }

    pub fn with_headers(mut self, headers: EventHeaders) -> Self {
        self.headers = headers;
        self
    }

    pub fn record(
        self,
        stream_id: impl Into<String>,
        stream_position: StreamPosition,
        recorded_at: chrono::DateTime<chrono::Utc>,
    ) -> StreamEvent {
        StreamEvent {
            stream_id: stream_id.into(),
            event: self,
            stream_position,
            recorded_at,
        }
    }

    pub fn decode<E>(&self, stream_id: &str) -> Result<E, E::Error>
    where
        E: EventDecode,
    {
        E::decode(&self.r#type, stream_id, &self.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventDecode, EventEncode, EventId, EventIdentity, EventType, StreamPosition};
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use std::str::FromStr;
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

        fn decode(_event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<Self, Self::Error> {
            decode_json(payload)
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
        let event = Event::from_domain_event(&payload).unwrap();

        assert_eq!(event.id.as_uuid().get_version_num(), 7);
        assert_eq!(event.r#type, "TestEvent");
        assert_eq!(event.decode::<TestEvent>("alpha").unwrap().value, "beta");
    }

    #[test]
    fn event_accepts_event_supplied_event_id() {
        let event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0001));
        let payload = IdentifiedEvent {
            id: event_id,
            value: "beta".to_string(),
        };
        let event = Event::from_domain_event(&payload).unwrap();

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
        let event = Event::from_domain_event(&payload).unwrap();

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
        let event = Event::from_domain_event(&payload).unwrap();
        assert_eq!(event.decode::<TestEvent>("alpha").unwrap().id, "alpha");

        let recorded = event.record(
            "alpha",
            position(1),
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        );
        assert_eq!(recorded.decode::<TestEvent>().unwrap().id, "alpha");
    }

    #[test]
    fn event_and_stream_event_round_trip_headers() {
        let event = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let headers = EventHeaders::one(HeaderKey::new("trace-id").unwrap(), "trace-1").unwrap();

        let generated = Event::from_domain_event(&event).unwrap().with_headers(headers.clone());
        assert_eq!(generated.decode::<TestEvent>("alpha").unwrap(), event);
        assert_eq!(generated.headers.get("trace-id"), Some("trace-1"));

        let no_headers = Event::from_domain_event(&event).unwrap();
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
