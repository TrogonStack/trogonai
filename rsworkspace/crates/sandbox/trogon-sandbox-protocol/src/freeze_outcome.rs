/// What a host did when asked to freeze a lease.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FreezeOutcome {
    /// The lease was frozen by this request.
    Frozen,
    /// The lease was already paused by someone else.
    AlreadyPaused,
    /// The host cannot freeze this lease.
    Unsupported,
    /// The host cannot act on this identity any more.
    Stale,
}
