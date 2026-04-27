#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use trogon_std::{NowV7, UuidV7Generator};
use uuid::Uuid;

mod decision;
mod execution;
pub mod nats;
pub mod snapshot;
mod stream;
pub mod testing;

pub use decision::{Act, Decide, Decision, NonEmpty, StateMachine, StreamCommand, decide};
pub use execution::{
    CommandExecution, CommandFailure, CommandInfraError, CommandResult, CommandSnapshotPolicy, ExecutionResult,
    FrequencySnapshot, NoSnapshot, SnapshotDecision, SnapshotPolicy, Snapshots, WithoutSnapshots,
};
pub use nats::snapshot_store::{
    SnapshotStoreError, checkpoint_key, list_snapshots, load_snapshot, load_snapshot_map, maybe_advance_checkpoint,
    persist_snapshot_change, read_checkpoint, snapshot_key, write_checkpoint,
};
pub use nats::streams::{
    StreamStoreError, TROGON_EVENT_TYPE, append_stream, read_stream_from, read_stream_range, record_stream_message,
};
pub use snapshot::{Snapshot, SnapshotChange, SnapshotRead, SnapshotSchema, SnapshotStoreConfig, SnapshotWrite};
pub use stream::{AppendOutcome, StreamAppend, StreamRead, StreamReadResult, StreamState};
pub use testing::{Decider, TestCase, ThenError, ThenEvents, ThenExpectation, Timeline, decider};

pub trait EventCodec<T> {
    type Error;

    fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error>;
    fn decode(&self, event_type: &str, stream_id: &str, payload: &[u8]) -> Result<T, Self::Error>;
}

pub trait CanonicalEventCodec: Sized {
    type Codec: EventCodec<Self>;

    fn canonical_codec() -> Self::Codec;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonEventCodec;

impl<T> EventCodec<T> for JsonEventCodec
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

pub trait EventType {
    fn event_type(&self) -> &'static str;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(Uuid);

impl EventId {
    pub fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub fn now_v7<N>(now_v7: &N) -> Self
    where
        N: NowV7 + ?Sized,
    {
        Self(now_v7.now_v7())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Debug for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EventId").field(&self.0).finish()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for EventId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl From<EventId> for Uuid {
    fn from(value: EventId) -> Self {
        value.0
    }
}

impl FromStr for EventId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self::new)
    }
}

impl Serialize for EventId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(value.as_str()).map_err(serde::de::Error::custom)
    }
}

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
        E: EventType + Serialize + DeserializeOwned,
    {
        Self::new_with_codec_and_generator(stream_id, &JsonEventCodec, &UuidV7Generator, event)
    }

    pub fn new_with_codec<E, C>(stream_id: impl AsRef<str>, codec: &C, event: E) -> Result<Self, C::Error>
    where
        E: EventType,
        C: EventCodec<E>,
    {
        Self::new_with_codec_and_generator(stream_id, codec, &UuidV7Generator, event)
    }

    pub fn new_with_event_id<E>(
        stream_id: impl AsRef<str>,
        event_id: impl Into<EventId>,
        event: E,
    ) -> serde_json::Result<Self>
    where
        E: EventType + Serialize + DeserializeOwned,
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
        E: EventType,
        C: EventCodec<E>,
    {
        Ok(Self {
            event_id: event_id.into(),
            event_type: event.event_type().to_string(),
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
        E: EventType,
        C: EventCodec<E>,
        N: NowV7,
    {
        Self::new_with_codec_and_event_id(stream_id, codec, EventId::now_v7(now_v7), event)
    }

    pub fn with_metadata<E, M>(stream_id: impl AsRef<str>, event: E, metadata: Option<M>) -> serde_json::Result<Self>
    where
        E: EventType + Serialize + DeserializeOwned,
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
        E: EventType,
        EC: EventCodec<E>,
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
        E: EventType + Serialize + DeserializeOwned,
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
        E: EventType,
        EC: EventCodec<E>,
        MC: EventCodec<M>,
    {
        Ok(Self {
            event_id: event_id.into(),
            event_type: event.event_type().to_string(),
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
        E: EventType,
        EC: EventCodec<E>,
        MC: EventCodec<M>,
        N: NowV7,
    {
        Self::with_codecs_and_event_id(
            stream_id,
            event_codec,
            metadata_codec,
            EventId::now_v7(now_v7),
            event,
            metadata,
        )
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

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestEvent {
        id: String,
        value: String,
    }

    impl EventType for TestEvent {
        fn event_type(&self) -> &'static str {
            "TestEvent"
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
}
