mod codec_error;
mod data;
mod recorded;

pub use codec_error::{EncodeEventError, EventDataEncodeError};
pub use data::EventData;
pub use recorded::RecordedEvent;

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

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestMetadata {
        trace_id: String,
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
            "stream-alpha",
            Some(position(2)),
            Some(10),
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.recorded_stream_id, "stream-alpha");
        assert_eq!(recorded.stream_position, Some(position(2)));
        assert_eq!(recorded.log_position, Some(10));
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

        let recorded = event.record(
            "stream-alpha",
            None,
            Some(42),
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        );
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
        let metadata = TestMetadata {
            trace_id: "trace-1".to_string(),
        };

        let generated = EventData::from_event("alpha", &TestEventCodec, &event)
            .unwrap()
            .with_metadata(&TestEventCodec, Some(&metadata))
            .unwrap();
        assert_eq!(
            generated.decode_data_with::<TestEvent, _>(&TestEventCodec).unwrap(),
            event
        );
        assert_eq!(
            generated
                .decode_metadata_with::<TestMetadata, _>(&TestEventCodec)
                .unwrap(),
            Some(metadata.clone())
        );

        let no_metadata = EventData::from_event("alpha", &TestEventCodec, &event)
            .unwrap()
            .with_metadata::<TestMetadata, _>(&TestEventCodec, None)
            .unwrap();
        assert_eq!(
            no_metadata
                .decode_metadata_with::<TestMetadata, _>(&TestEventCodec)
                .unwrap(),
            None
        );

        let recorded = RecordedEvent {
            event_id: generated.event_id,
            event_type: generated.event_type.clone(),
            event_stream_id: generated.stream_id.clone(),
            payload: generated.payload.clone(),
            metadata: generated.metadata.clone(),
            recorded_stream_id: "recorded-alpha".to_string(),
            stream_position: Some(position(7)),
            log_position: Some(9),
            recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_002, 0).unwrap(),
        };

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.recorded_stream_id, "recorded-alpha");
        assert_eq!(recorded.stream_position, Some(position(7)));
        assert_eq!(recorded.log_position, Some(9));
        assert_eq!(
            recorded.decode_data_with::<TestEvent, _>(&TestEventCodec).unwrap(),
            event
        );
        assert_eq!(
            recorded
                .decode_metadata_with::<TestMetadata, _>(&TestEventCodec)
                .unwrap(),
            Some(metadata)
        );
        assert_eq!(recorded.subject_with_prefix("events.test."), "events.test.alpha");
    }
}
