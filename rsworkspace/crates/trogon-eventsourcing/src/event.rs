use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use trogon_std::{NowV7, UuidV7Generator};

use crate::{EventCodec, EventEnvelopeCodec, EventId, EventIdentity, EventType, JsonEventCodec};

#[derive(Debug, Clone, PartialEq)]
pub struct EventData {
    pub event_id: EventId,
    pub event_type: String,
    pub stream_id: String,
    pub payload: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedEvent {
    pub event_id: EventId,
    pub event_type: String,
    pub event_stream_id: String,
    pub payload: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
    pub recorded_stream_id: String,
    pub stream_position: Option<u64>,
    pub log_position: Option<u64>,
    pub recorded_at: DateTime<Utc>,
}

impl EventData {
    pub fn new<E>(stream_id: impl AsRef<str>, event: E) -> serde_json::Result<Self>
    where
        E: EventType + EventIdentity + Serialize + DeserializeOwned,
    {
        Self::new_with_codec_and_generator(stream_id, &JsonEventCodec, &UuidV7Generator, event)
    }

    pub fn new_with_codec<E, C>(stream_id: impl AsRef<str>, codec: &C, event: E) -> Result<Self, C::Error>
    where
        C: EventEnvelopeCodec<E>,
    {
        Self::new_with_codec_and_generator(stream_id, codec, &UuidV7Generator, event)
    }

    pub fn new_with_event_id<E>(
        stream_id: impl AsRef<str>,
        event_id: impl Into<EventId>,
        event: E,
    ) -> serde_json::Result<Self>
    where
        E: EventType + EventIdentity + Serialize + DeserializeOwned,
    {
        Self::new_with_codec_and_event_id(stream_id, &JsonEventCodec, event_id, event)
    }

    pub fn new_with_codec_and_event_id<E, C>(
        stream_id: impl AsRef<str>,
        codec: &C,
        event_id: impl Into<EventId>,
        event: E,
    ) -> Result<Self, C::Error>
    where
        C: EventEnvelopeCodec<E>,
    {
        Ok(Self {
            event_id: event_id.into(),
            event_type: codec.event_type(&event)?.to_string(),
            stream_id: stream_id.as_ref().to_string(),
            payload: codec.encode(&event)?,
            metadata: None,
        })
    }

    pub fn new_with_codec_and_generator<E, C, N>(
        stream_id: impl AsRef<str>,
        codec: &C,
        now_v7: &N,
        event: E,
    ) -> Result<Self, C::Error>
    where
        C: EventEnvelopeCodec<E>,
        N: NowV7,
    {
        let event_id = codec.event_id(&event).unwrap_or_else(|| EventId::now_v7(now_v7));
        Self::new_with_codec_and_event_id(stream_id, codec, event_id, event)
    }

    pub fn with_metadata<E, M>(stream_id: impl AsRef<str>, event: E, metadata: Option<M>) -> serde_json::Result<Self>
    where
        E: EventType + EventIdentity + Serialize + DeserializeOwned,
        M: Serialize + DeserializeOwned,
    {
        Self::with_codecs_and_generator(
            stream_id,
            &JsonEventCodec,
            &JsonEventCodec,
            &UuidV7Generator,
            event,
            metadata,
        )
        .map_err(|error| match error {
            CodecError::Data(source) | CodecError::Metadata(source) => source,
        })
    }

    pub fn with_codecs<E, M, EC, MC>(
        stream_id: impl AsRef<str>,
        event_codec: &EC,
        metadata_codec: &MC,
        event: E,
        metadata: Option<M>,
    ) -> Result<Self, CodecError<EC::Error, MC::Error>>
    where
        EC: EventEnvelopeCodec<E>,
        MC: EventCodec<M>,
    {
        Self::with_codecs_and_generator(
            stream_id,
            event_codec,
            metadata_codec,
            &UuidV7Generator,
            event,
            metadata,
        )
    }

    pub fn with_metadata_and_event_id<E, M>(
        stream_id: impl AsRef<str>,
        event_id: impl Into<EventId>,
        event: E,
        metadata: Option<M>,
    ) -> serde_json::Result<Self>
    where
        E: EventType + EventIdentity + Serialize + DeserializeOwned,
        M: Serialize + DeserializeOwned,
    {
        Self::with_codecs_and_event_id(stream_id, &JsonEventCodec, &JsonEventCodec, event_id, event, metadata).map_err(
            |error| match error {
                CodecError::Data(source) | CodecError::Metadata(source) => source,
            },
        )
    }

    pub fn with_codecs_and_event_id<E, M, EC, MC>(
        stream_id: impl AsRef<str>,
        event_codec: &EC,
        metadata_codec: &MC,
        event_id: impl Into<EventId>,
        event: E,
        metadata: Option<M>,
    ) -> Result<Self, CodecError<EC::Error, MC::Error>>
    where
        EC: EventEnvelopeCodec<E>,
        MC: EventCodec<M>,
    {
        Ok(Self {
            event_id: event_id.into(),
            event_type: event_codec.event_type(&event).map_err(CodecError::Data)?.to_string(),
            stream_id: stream_id.as_ref().to_string(),
            payload: event_codec.encode(&event).map_err(CodecError::Data)?,
            metadata: metadata
                .map(|value| metadata_codec.encode(&value))
                .transpose()
                .map_err(CodecError::Metadata)?,
        })
    }

    pub fn with_codecs_and_generator<E, M, EC, MC, N>(
        stream_id: impl AsRef<str>,
        event_codec: &EC,
        metadata_codec: &MC,
        now_v7: &N,
        event: E,
        metadata: Option<M>,
    ) -> Result<Self, CodecError<EC::Error, MC::Error>>
    where
        EC: EventEnvelopeCodec<E>,
        MC: EventCodec<M>,
        N: NowV7,
    {
        let event_id = event_codec.event_id(&event).unwrap_or_else(|| EventId::now_v7(now_v7));
        Self::with_codecs_and_event_id(stream_id, event_codec, metadata_codec, event_id, event, metadata)
    }

    pub fn record(
        self,
        recorded_stream_id: impl Into<String>,
        stream_position: Option<u64>,
        log_position: Option<u64>,
        recorded_at: DateTime<Utc>,
    ) -> RecordedEvent {
        RecordedEvent {
            event_id: self.event_id,
            event_type: self.event_type,
            event_stream_id: self.stream_id,
            payload: self.payload,
            metadata: self.metadata,
            recorded_stream_id: recorded_stream_id.into(),
            stream_position,
            log_position,
            recorded_at,
        }
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }

    pub fn decode_data<E>(&self) -> serde_json::Result<E>
    where
        E: Serialize + DeserializeOwned,
    {
        self.decode_data_with(&JsonEventCodec)
    }

    pub fn decode_data_with<E, C>(&self, codec: &C) -> Result<E, C::Error>
    where
        C: EventCodec<E>,
    {
        codec.decode(&self.event_type, &self.stream_id, &self.payload)
    }

    pub fn decode_metadata<M>(&self) -> serde_json::Result<Option<M>>
    where
        M: Serialize + DeserializeOwned,
    {
        self.decode_metadata_with(&JsonEventCodec)
    }

    pub fn decode_metadata_with<M, C>(&self, codec: &C) -> Result<Option<M>, C::Error>
    where
        C: EventCodec<M>,
    {
        self.metadata
            .as_deref()
            .map(|value| codec.decode(&self.event_type, &self.stream_id, value))
            .transpose()
    }
}

impl RecordedEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_id: EventId,
        event_type: impl Into<String>,
        event_stream_id: impl Into<String>,
        payload: Vec<u8>,
        metadata: Option<Vec<u8>>,
        recorded_stream_id: impl Into<String>,
        stream_position: Option<u64>,
        log_position: Option<u64>,
        recorded_at: DateTime<Utc>,
    ) -> Self {
        Self {
            event_id,
            event_type: event_type.into(),
            event_stream_id: event_stream_id.into(),
            payload,
            metadata,
            recorded_stream_id: recorded_stream_id.into(),
            stream_position,
            log_position,
            recorded_at,
        }
    }

    pub fn stream_id(&self) -> &str {
        &self.event_stream_id
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }

    pub fn decode_data<E>(&self) -> serde_json::Result<E>
    where
        E: Serialize + DeserializeOwned,
    {
        self.decode_data_with(&JsonEventCodec)
    }

    pub fn decode_data_with<E, C>(&self, codec: &C) -> Result<E, C::Error>
    where
        C: EventCodec<E>,
    {
        codec.decode(&self.event_type, &self.event_stream_id, &self.payload)
    }

    pub fn decode_metadata<M>(&self) -> serde_json::Result<Option<M>>
    where
        M: Serialize + DeserializeOwned,
    {
        self.decode_metadata_with(&JsonEventCodec)
    }

    pub fn decode_metadata_with<M, C>(&self, codec: &C) -> Result<Option<M>, C::Error>
    where
        C: EventCodec<M>,
    {
        self.metadata
            .as_deref()
            .map(|value| codec.decode(&self.event_type, &self.event_stream_id, value))
            .transpose()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError<DataError, MetadataError> {
    Data(DataError),
    Metadata(MetadataError),
}

#[cfg(test)]
mod tests {
    use super::*;
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
