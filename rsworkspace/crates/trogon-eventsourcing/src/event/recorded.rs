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
