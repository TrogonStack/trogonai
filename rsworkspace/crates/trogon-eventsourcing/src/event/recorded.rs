use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};

use crate::{EventCodec, EventId, JsonEventCodec};

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
