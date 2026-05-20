use chrono::{DateTime, Utc};

use crate::{Event, EventData, EventDecode, StreamPosition};

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

    pub fn decode<E>(&self) -> Result<E, E::Error>
    where
        E: EventDecode,
    {
        E::decode(EventData::new(&self.event.r#type, &self.event.content))
    }
}
