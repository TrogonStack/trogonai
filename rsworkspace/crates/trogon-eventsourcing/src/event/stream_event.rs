use chrono::{DateTime, Utc};

use crate::{Event, EventDecode, StreamPosition};

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

    pub fn decode<E>(&self) -> Result<E, E::Error>
    where
        E: EventDecode,
    {
        self.event.decode(&self.stream_id)
    }
}
