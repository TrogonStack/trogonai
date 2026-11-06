use std::time::SystemTime;

use crate::agent_id::AgentId;
use crate::destination::Destination;
use crate::policy_source::PolicySource;

/// What a snapshot can say about one `(agent, destination)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotVerdict {
    Permitted,
    NotPermitted,
    /// The snapshot cannot vouch for an answer right now (stale, not yet
    /// loaded). Never reported as a denial: operational unavailability and a
    /// security refusal stay distinguishable (ADR#0085 Decision 5).
    Unavailable,
}

/// The read side of policy. The listener, the destination rules and the audit
/// path see policy only through this trait, so the NATS KV projection of
/// ADR#0085 Decision 4 replaces the static file without touching any of them.
///
/// `lookup` is synchronous and answers from memory: the proxy decides on its
/// own projection, never by calling out per connection.
pub trait PolicySnapshot: Send + Sync {
    fn source(&self) -> &PolicySource;

    /// Whether `agent` has a permit equal to `destination`. Equality is exact:
    /// same host, same port, no patterns, no inheritance from a broader entry.
    /// `now` lets an implementation measure its own age.
    fn lookup(&self, agent: &AgentId, destination: &Destination, now: SystemTime) -> SnapshotVerdict;
}
