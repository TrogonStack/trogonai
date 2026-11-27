use std::time::Duration;

use trogon_sandbox_contract::LeaseId;

/// Outcome of an atomic name claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameClaim {
    /// The name now maps to the claiming lease (also returned when a retry of
    /// the same claim finds its own mapping).
    Claimed,
    /// Another lease already holds the name; `age` is how long ago that
    /// mapping was written. `pending` is true while the holder's claim still
    /// carries an unexpired pending marker, meaning its request is still
    /// provisioning. An expired marker reads as not pending so a crashed
    /// claimer cannot block the name forever.
    Held {
        lease: LeaseId,
        age: Duration,
        pending: bool,
    },
}
