use chrono::{DateTime, Utc};
use trogon_std::UuidV7Generator;

use super::{EventMetadata, RecordedEvent};
use crate::{EncodeEventError, EventCodec, EventDataEncodeError, EventId, EventIdentity, EventType, StreamPosition};

#[derive(Debug, Clone, PartialEq)]
pub struct EventData {
    pub event_id: EventId,
    pub event_type: String,
    pub stream_id: String,
    pub payload: Vec<u8>,
    pub metadata: EventMetadata,
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
            metadata: EventMetadata::empty(),
        })
    }

    pub fn with_metadata(mut self, metadata: EventMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn record(self, stream_position: Option<StreamPosition>, recorded_at: DateTime<Utc>) -> RecordedEvent {
        RecordedEvent {
            event_id: self.event_id,
            event_type: self.event_type,
            event_stream_id: self.stream_id,
            payload: self.payload,
            metadata: self.metadata,
            stream_position,
            recorded_at,
        }
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }

    pub fn decode_data_with<E, C>(&self, codec: &C) -> Result<E, C::Error>
    where
        C: EventCodec<E>,
    {
        codec.decode(&self.event_type, &self.stream_id, &self.payload)
    }
}
