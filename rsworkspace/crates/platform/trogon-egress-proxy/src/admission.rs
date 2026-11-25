//! The evaluation seam (ADR#0085 Decision 6): `admit(identity, destination,
//! now)` is pure, synchronous and knows nothing about sockets, TLS or the
//! source of the snapshot. An external-authorization adapter can call the
//! same function.

use std::time::SystemTime;

use crate::agent_id::AgentId;
use crate::destination::Destination;
use crate::fault::Fault;
use crate::policy_snapshot::{PolicySnapshot, SnapshotVerdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Permit,
    Refuse(Fault),
}

/// Admits `destination` for `identity` when the snapshot holds a permit equal
/// to it. The default is deny: an agent the snapshot does not know has no
/// permit, and a snapshot that cannot answer is `POLICY_UNAVAILABLE`, not a
/// denial.
///
/// An IP-literal destination is a single exact comparison like any other, so
/// it is admitted only when the permit is that exact literal (Decision 7).
pub fn admit<S>(snapshot: &S, identity: &AgentId, destination: &Destination, now: SystemTime) -> Admission
where
    S: PolicySnapshot + ?Sized,
{
    match snapshot.lookup(identity, destination, now) {
        SnapshotVerdict::Permitted => Admission::Permit,
        SnapshotVerdict::NotPermitted => Admission::Refuse(Fault::NoPermit),
        SnapshotVerdict::Unavailable => Admission::Refuse(Fault::PolicyUnavailable),
    }
}

#[cfg(test)]
mod tests;
