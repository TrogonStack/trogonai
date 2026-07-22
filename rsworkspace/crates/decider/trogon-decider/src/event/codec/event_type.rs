/// Provides the stable name stored with an event payload.
///
/// The returned name is part of the persistence contract. It should remain
/// stable across deployments and must not depend on runtime data.
pub trait EventType {
    /// Error returned when the event type cannot be resolved.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Returns the persistent event type name.
    fn event_type(&self) -> Result<&'static str, Self::Error>;
}
