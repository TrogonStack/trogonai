/// An [`EntityState`](crate::EntityState) without its payload, for tables and metrics.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum EntityStateKind {
    Creating,
    Starting,
    Running,
    Paused,
    Stopping,
    Stopped,
    Deleting,
    Failed,
}
