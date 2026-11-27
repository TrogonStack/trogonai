/// What a host did when asked to purge a lease.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PurgeOutcome {
    /// The backend resource was cleaned up.
    Purged,
    /// Exact cleanup or supersession was already confirmed externally.
    Settled,
    /// The host cannot act on this identity and has no settlement proof.
    Stale,
}
