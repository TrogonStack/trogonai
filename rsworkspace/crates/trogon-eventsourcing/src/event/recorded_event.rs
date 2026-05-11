use chrono::{DateTime, Utc};

use crate::{EventCodec, EventId, EventMetadata, StreamPosition};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedEvent {
    pub event_id: EventId,
    pub event_type: String,
    pub event_stream_id: String,
    pub payload: Vec<u8>,
    pub metadata: EventMetadata,
    pub stream_position: Option<StreamPosition>,
    pub recorded_at: DateTime<Utc>,
}

impl RecordedEvent {
    pub fn stream_id(&self) -> &str {
        &self.event_stream_id
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }

    pub fn decode_data_with<E, C>(&self, codec: &C) -> Result<E, C::Error>
    where
        C: EventCodec<E>,
    {
        codec.decode(&self.event_type, &self.event_stream_id, &self.payload)
    }
}
