//! The delivery envelope a durable frame carries.

/// Where a durable message came from in the server's event log: its event id and position, and when and by whom it
/// was written. A message passed on to the consumer keeps this so the consumer can report how far it has read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseCcrEvent {
    /// The server's id for the event.
    pub event_id: String,
    /// The event's position in the log.
    pub sequence_num: u64,
    /// When the event was created, when the server said so.
    pub created_at: Option<String>,
    /// Who wrote the event, when the server said so.
    pub source: Option<String>,
}
