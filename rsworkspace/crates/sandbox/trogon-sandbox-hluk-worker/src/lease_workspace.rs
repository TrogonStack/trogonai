/// The host directory a lease mounts at `/workspace`, and who it belongs to.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LeaseWorkspace {
    /// Created by the worker for this lease, writable, removed when the lease ends.
    Scratch,
    /// The caller's host directory, mounted read-only and left in place.
    CallerReadOnly,
}
