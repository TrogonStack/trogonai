use chrono::{DateTime, Utc};

use crate::{Event, EventCodec, StreamPosition};

#[derive(Debug, Clone, PartialEq)]
pub struct StreamEvent {
    pub stream_id: String,
    pub event: Event,
    pub stream_position: StreamPosition,
    pub recorded_at: DateTime<Utc>,
}

impl StreamEvent {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn subject_with_prefix(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.stream_id())
    }

    pub fn decode_with<E, C>(&self, codec: &C) -> Result<E, C::Error>
    where
        C: EventCodec<E>,
    {
        self.event.decode_with(&self.stream_id, codec)
    }
}
