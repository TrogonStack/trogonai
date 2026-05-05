use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use trogon_std::{NowV7, UuidV7Generator};

use super::{CodecError, RecordedEvent};
use crate::{EventCodec, EventEnvelopeCodec, EventId, EventIdentity, EventType, JsonEventCodec};

#[derive(Debug, Clone, PartialEq)]
pub struct EventData {
    pub event_id: EventId,
    pub event_type: String,
    pub stream_id: String,
    pub payload: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
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

    pub fn new_with_codec_ref<E, C>(stream_id: impl AsRef<str>, codec: &C, event: &E) -> Result<Self, C::Error>
    where
        C: EventEnvelopeCodec<E>,
    {
        let event_id = codec
            .event_id(event)
            .unwrap_or_else(|| EventId::now_v7(&UuidV7Generator));
        Ok(Self {
            event_id,
            event_type: codec.event_type(event)?.to_string(),
            stream_id: stream_id.as_ref().to_string(),
            payload: codec.encode(event)?,
            metadata: None,
        })
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
