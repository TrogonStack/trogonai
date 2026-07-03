//! Event envelope types and serialization traits.
//!
//! Domain events stay owned by application crates. This module defines the
//! storage-facing envelope and the traits adapters need to turn domain events
//! into bytes without depending on a specific serializer.
//!
//! Decoding also carries the event-set ownership decision. A codec can skip an
//! envelope whose stored event type belongs to another decider, while still
//! returning an error for payloads that claim to be part of its own event set
//! but cannot be decoded.

mod event_id;
mod event_identity;
mod stream_event;

use crate::headers::Headers;

pub use event_id::EventId;
pub use event_identity::EventIdentity;
pub use stream_event::StreamEvent;
pub use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Unique identity assigned to this event occurrence.
    pub id: EventId,
    /// Stable domain event name used to select the decoder.
    pub r#type: String,
    /// Serialized domain event payload.
    pub content: Vec<u8>,
    /// Metadata propagated with the event but kept outside the payload.
    pub headers: Headers,
}

#[cfg(test)]
mod tests;
