use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use trogon_std::UuidV7Generator;

use super::{EncodeEventError, EventDataEncodeError, RecordedEvent};
use crate::{EventCodec, EventId, EventIdentity, EventType, JsonEventCodec};

#[derive(Debug, Clone, PartialEq)]
pub struct EventData {
    pub event_id: EventId,
    pub event_type: String,
    pub stream_id: String,
    pub payload: Vec<u8>,
    pub metadata: Option<Vec<u8>>,
}

impl EventData {
    pub fn from_event<E, C>(
        stream_id: impl AsRef<str>,
        codec: &C,
        event: &E,
    ) -> Result<Self, EventDataEncodeError<E, C>>
    where
        E: EventType + EventIdentity,
        C: EventCodec<E>,
    {
        let event_id = event.event_id().unwrap_or_else(|| EventId::now_v7(&UuidV7Generator));
        Ok(Self {
            event_id,
            event_type: event.event_type().map_err(EncodeEventError::EventType)?.to_string(),
            stream_id: stream_id.as_ref().to_string(),
            payload: codec.encode(event).map_err(EncodeEventError::EventCodec)?,
            metadata: None,
        })
    }

    pub fn with_metadata<M, C>(mut self, codec: &C, metadata: Option<&M>) -> Result<Self, C::Error>
    where
        C: EventCodec<M>,
    {
        self.metadata = metadata.map(|value| codec.encode(value)).transpose()?;
        Ok(self)
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
