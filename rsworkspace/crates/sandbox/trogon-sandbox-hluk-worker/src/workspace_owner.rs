/// Who removes the workspace directory when the lease ends.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WorkspaceOwner {
    /// The worker created it for this lease.
    Worker,
    /// It is the caller's host directory.
    Caller,
}
