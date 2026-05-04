mod codec_error;
mod data;
mod recorded;

pub use codec_error::CodecError;
pub use data::EventData;
pub use recorded::RecordedEvent;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventId, EventIdentity, EventType, JsonEventCodec};
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};
    use std::str::FromStr;
    use uuid::Uuid;

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
        fn event_type(&self) -> &'static str {
            "TestEvent"
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
        fn event_type(&self) -> &'static str {
            "IdentifiedEvent"
        }
    }

    #[test]
    fn event_data_uses_event_traits() {
        let event = EventData::new(
            "alpha",
            TestEvent {
                id: "alpha".to_string(),
                value: "beta".to_string(),
            },
        )
        .unwrap();

        assert_eq!(event.stream_id(), "alpha");
        assert_eq!(event.event_id.as_uuid().get_version_num(), 7);
        assert_eq!(event.event_type, "TestEvent");
        assert_eq!(event.subject_with_prefix("events.test."), "events.test.alpha");
        assert_eq!(event.decode_data::<TestEvent>().unwrap().value, "beta");
    }

    #[test]
    fn event_data_accepts_caller_supplied_uuid_event_id() {
        let event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0001));
        let event = EventData::new_with_event_id(
            "alpha",
            event_id,
            TestEvent {
                id: "alpha".to_string(),
                value: "beta".to_string(),
            },
        )
        .unwrap();

        assert_eq!(event.event_id, event_id);
    }

    #[test]
    fn event_data_accepts_event_supplied_event_id() {
        let event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0002));
        let event = EventData::new(
            "alpha",
            IdentifiedEvent {
                id: event_id,
                value: "beta".to_string(),
            },
        )
        .unwrap();

        assert_eq!(event.event_id, event_id);
    }

    #[test]
    fn caller_supplied_event_id_wins_over_event_identity() {
        let event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0003));
        let explicit_event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0004));
        let event = EventData::new_with_event_id(
            "alpha",
            explicit_event_id,
            IdentifiedEvent {
                id: event_id,
                value: "beta".to_string(),
            },
        )
        .unwrap();

        assert_eq!(event.event_id, explicit_event_id);
    }

    #[test]
    fn event_data_rejects_non_uuid_event_ids() {
        assert!(EventId::from_str("not-a-uuid").is_err());
    }

    #[test]
    fn recorded_event_preserves_store_context() {
        let event = EventData::new(
            "alpha",
            TestEvent {
                id: "alpha".to_string(),
                value: "beta".to_string(),
            },
        )
        .unwrap();

        let recorded = event.record(
            "stream-alpha",
            Some(2),
            Some(10),
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.recorded_stream_id, "stream-alpha");
        assert_eq!(recorded.stream_position, Some(2));
        assert_eq!(recorded.log_position, Some(10));
        assert_eq!(recorded.subject_with_prefix("events.test."), "events.test.alpha");
    }

    #[test]
    fn event_data_and_recorded_event_decode_payloads() {
        let event = EventData::new(
            "alpha",
            TestEvent {
                id: "alpha".to_string(),
                value: "beta".to_string(),
            },
        )
        .unwrap();
        assert_eq!(event.decode_data::<TestEvent>().unwrap().id, "alpha");

        let recorded = event.record(
            "stream-alpha",
            None,
            Some(42),
            DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        );
        assert_eq!(recorded.decode_data::<TestEvent>().unwrap().id, "alpha");
    }

    #[test]
    fn event_data_and_recorded_event_round_trip_metadata() {
        let event_id = EventId::from(Uuid::from_u128(0x018f_8f4d_94a8_7000_8000_0000_0000_0005));
        let event = TestEvent {
            id: "alpha".to_string(),
            value: "beta".to_string(),
        };
        let metadata = TestMetadata {
            trace_id: "trace-1".to_string(),
        };

        let generated = EventData::with_metadata("alpha", event.clone(), Some(metadata.clone())).unwrap();
        assert_eq!(generated.decode_data::<TestEvent>().unwrap(), event);
        assert_eq!(
            generated.decode_metadata::<TestMetadata>().unwrap(),
            Some(metadata.clone())
        );

        let with_codecs = EventData::with_codecs(
            "alpha",
            &JsonEventCodec,
            &JsonEventCodec,
            event.clone(),
            Some(metadata.clone()),
        )
        .unwrap();
        assert_eq!(
            with_codecs.decode_data_with::<TestEvent, _>(&JsonEventCodec).unwrap(),
            event
        );
        assert_eq!(
            with_codecs
                .decode_metadata_with::<TestMetadata, _>(&JsonEventCodec)
                .unwrap(),
            Some(metadata.clone())
        );

        let explicit =
            EventData::with_metadata_and_event_id("alpha", event_id, event.clone(), Some(metadata.clone())).unwrap();
        assert_eq!(explicit.event_id, event_id);

        let no_metadata = EventData::with_codecs_and_event_id(
            "alpha",
            &JsonEventCodec,
            &JsonEventCodec,
            event_id,
            event.clone(),
            None::<TestMetadata>,
        )
        .unwrap();
        assert_eq!(no_metadata.decode_metadata::<TestMetadata>().unwrap(), None);

        let recorded = RecordedEvent::new(
            explicit.event_id,
            explicit.event_type.clone(),
            explicit.stream_id.clone(),
            explicit.payload.clone(),
            explicit.metadata.clone(),
            "recorded-alpha",
            Some(7),
            Some(9),
            DateTime::<Utc>::from_timestamp(1_700_000_002, 0).unwrap(),
        );

        assert_eq!(recorded.stream_id(), "alpha");
        assert_eq!(recorded.recorded_stream_id, "recorded-alpha");
        assert_eq!(recorded.stream_position, Some(7));
        assert_eq!(recorded.log_position, Some(9));
        assert_eq!(recorded.decode_data::<TestEvent>().unwrap(), event);
        assert_eq!(recorded.decode_metadata::<TestMetadata>().unwrap(), Some(metadata));
        assert_eq!(recorded.subject_with_prefix("events.test."), "events.test.alpha");
    }
}
