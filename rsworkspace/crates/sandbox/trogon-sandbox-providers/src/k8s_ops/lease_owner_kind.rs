/// The kind of object that owns a lease's Secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseOwnerKind {
    /// A cold lease's Job.
    Job,
    /// A claimed warm pod.
    Pod,
}
