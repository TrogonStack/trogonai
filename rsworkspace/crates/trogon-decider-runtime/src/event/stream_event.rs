use chrono::{DateTime, Utc};

use crate::{Event, EventData, EventDecode, EventDecodeOutcome, StreamPosition};

/// Event envelope returned from a concrete stream.
///
/// `StreamEvent` adds the stream-local context needed for replay, projections,
/// and freshness checks while keeping the original [`Event`] envelope intact.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamEvent {
    /// Stream that produced the event.
    pub stream_id: String,
    /// Stored event envelope.
    pub event: Event,
    /// Comparable high-watermark observed for this event in its stream.
    pub stream_position: StreamPosition,
    /// Store timestamp for the persisted event.
    pub recorded_at: DateTime<Utc>,
}

impl StreamEvent {
    /// Returns the stream identity as a borrowed string.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Decodes the enclosed event payload as a domain event.
    pub fn decode<E>(&self) -> Result<EventDecodeOutcome<E>, E::Error>
    where
        E: EventDecode,
    {
        E::decode(EventData::new(&self.event.r#type, &self.event.content))
    }
}
