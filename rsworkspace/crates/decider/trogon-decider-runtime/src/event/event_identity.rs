use super::EventId;

/// Optional identity supplied by a domain event before it is enveloped.
///
/// Most events can let the runtime assign an [`EventId`]. Implement this when
/// a domain workflow already produced a durable event identity that must be
/// preserved through persistence.
pub trait EventIdentity {
    /// Returns the event identity to preserve, or `None` to let the caller
    /// assign one.
    fn event_id(&self) -> Option<EventId> {
        None
    }
}
