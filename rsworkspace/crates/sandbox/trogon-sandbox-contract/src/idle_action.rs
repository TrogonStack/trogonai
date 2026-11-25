use serde::{Deserialize, Serialize};

/// What an idle stage does to a lease.
///
/// Variants are ordered by severity: a lease never moves from a more severe stage back to a milder
/// one.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleAction {
    /// Freeze the lease through the normal pause verb.
    Pause,
    /// Suspend the lease through the normal suspend path, releasing its host resources while keeping
    /// a durable artifact. A provider that cannot suspend the lease gets a pause instead; one that
    /// cannot pause either skips the stage. Any later access wakes the lease.
    Suspend,
    /// Release the lease with the idle timeout release reason.
    Release,
}
