mod event_data;
mod event_metadata;
mod event_metadata_error;
mod metadata_key;
mod recorded_event;

pub use event_data::EventData;
pub use event_metadata::EventMetadata;
pub use event_metadata_error::EventMetadataError;
pub use metadata_key::MetadataKey;
pub use recorded_event::RecordedEvent;

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
    fn event_data_uses_event_traits() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = EventData::from_event("alpha", &TestEventCodec, &payload).unwrap();

        assert_eq!(event.stream_id(), "alpha");
        assert_eq!(event.event_id.as_uuid().get_version_num(), 7);
        assert_eq!(event.event_type, "TestEvent");
        assert_eq!(event.subject_with_prefix("events.test."), "events.test.alpha");
        assert_eq!(
            event.decode_data_with::<TestEvent, _>(&TestEventCodec).unwrap().value,
            "beta"
        );
    }

    #[test]
    fn event_data_accepts_event_supplied_event_id() {
        let event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0001));
        let payload = IdentifiedEvent {
            id: event_id,
            value: "beta".to_string(),
        };
        let event = EventData::from_event("alpha", &TestEventCodec, &payload).unwrap();

        assert_eq!(event.event_id, event_id);
    }

    #[test]
    fn event_data_rejects_non_uuid_event_ids() {
        assert!(EventId::from_str("not-a-uuid").is_err());
    }

    #[test]
    fn recorded_event_preserves_store_context() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = EventData::from_event("alpha", &TestEventCodec, &payload).unwrap();

        let recorded = event.record(
            Some(position(2)),
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_position, Some(position(2)));
        assert_eq!(recorded.subject_with_prefix("events.test."), "events.test.alpha");
    }

    #[test]
    fn event_data_and_recorded_event_decode_payloads() {
        let payload = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let event = EventData::from_event("alpha", &TestEventCodec, &payload).unwrap();
        assert_eq!(
            event.decode_data_with::<TestEvent, _>(&TestEventCodec).unwrap().id,
            "alpha"
        );

        let recorded = event.record(None, DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap());
        assert_eq!(
            recorded.decode_data_with::<TestEvent, _>(&TestEventCodec).unwrap().id,
            "alpha"
        );
    }

    #[test]
    fn event_data_and_recorded_event_round_trip_metadata() {
        let event = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let metadata = EventMetadata::one(MetadataKey::new("trace-id").unwrap(), "trace-1").unwrap();

        let generated = EventData::from_event("alpha", &TestEventCodec, &event)
            .unwrap()
            .with_metadata(metadata.clone());
        assert_eq!(
            generated.decode_data_with::<TestEvent, _>(&TestEventCodec).unwrap(),
            event
        );
        assert_eq!(generated.metadata.get("trace-id"), Some("trace-1"));

        let no_metadata = EventData::from_event("alpha", &TestEventCodec, &event).unwrap();
        assert!(no_metadata.metadata.is_empty());

        let recorded = RecordedEvent {
            event_id: generated.event_id,
            event_type: generated.event_type.clone(),
            event_stream_id: generated.stream_id.clone(),
            payload: generated.payload.clone(),
            metadata: generated.metadata.clone(),
            stream_position: Some(position(7)),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_002, 0).unwrap(),
        };

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.stream_position, Some(position(7)));
        assert_eq!(
            recorded.decode_data_with::<TestEvent, _>(&TestEventCodec).unwrap(),
            event
        );
        assert_eq!(recorded.metadata, metadata);
        assert_eq!(recorded.subject_with_prefix("events.test."), "events.test.alpha");
    }

    #[test]
    fn event_metadata_validates_header_safe_names_and_values() {
        let key = MetadataKey::new("trace-id").unwrap();
        assert_eq!(key.as_str(), "trace-id");

        assert_eq!(MetadataKey::new("").unwrap_err(), EventMetadataError::EmptyName);
        assert!(matches!(
            MetadataKey::new("Nats-Expected-Last-Subject-Sequence"),
            Err(EventMetadataError::ReservedName { .. })
        ));
        assert!(matches!(
            MetadataKey::new("trace id"),
            Err(EventMetadataError::InvalidName { .. })
        ));
        assert!(matches!(
            EventMetadata::one(MetadataKey::new("trace-id").unwrap(), "line\r\nbreak"),
            Err(EventMetadataError::InvalidValue { .. })
        ));
    }
}
