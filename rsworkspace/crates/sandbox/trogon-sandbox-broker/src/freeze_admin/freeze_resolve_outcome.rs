#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FreezeResolveOutcome {
    Applied,
    NoSuchLease,
    NoMarker,
    /// The marker is still within its attempts and the reconciler owns it.
    NotUnsettled {
        settle_attempts: u32,
    },
    /// The record kept changing under the operator; run the command again.
    Conflict,
}
