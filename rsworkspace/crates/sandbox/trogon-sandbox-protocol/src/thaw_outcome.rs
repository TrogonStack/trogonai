/// What a host did when asked to thaw a lease.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ThawOutcome {
    /// The lease was thawed.
    Thawed,
    /// The guard no longer owns a freeze on the lease.
    NoLongerOwned,
    /// The host cannot act on this identity yet; try again.
    Stale,
}
