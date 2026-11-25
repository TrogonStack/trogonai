//! Which resolved addresses the proxy may dial (ADR#0085 Decision 7).
//!
//! The caller resolves the name once and passes every candidate here. Only
//! addresses returned by this function are connected to, and nothing resolves
//! again, so an answer that changes between this check and the connect has no
//! effect.

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU16;
use std::time::SystemTime;

use crate::address_class::classify;
use crate::admission::{Admission, admit};
use crate::agent_id::AgentId;
use crate::destination::Destination;
use crate::destination_host::DestinationHost;
use crate::fault::Fault;
use crate::policy_snapshot::PolicySnapshot;

/// Filters `candidates`, in resolver order and without duplicates, down to the
/// addresses the policy intended.
///
/// A public address always qualifies. Any other class (loopback, link-local
/// including the metadata address, RFC 1918, CGNAT, unique-local, unspecified,
/// multicast, reserved) qualifies only when `agent` holds a permit that is
/// that exact address literal on `port`, which keeps platform-internal
/// services unreachable unless an operator names them.
///
/// Returns [`Fault::AddressForbidden`] when nothing qualifies, and
/// [`Fault::PolicyUnavailable`] when the snapshot cannot answer.
pub fn dial_targets<S>(
    snapshot: &S,
    agent: &AgentId,
    port: NonZeroU16,
    candidates: &[IpAddr],
    now: SystemTime,
) -> Result<Vec<SocketAddr>, Fault>
where
    S: PolicySnapshot + ?Sized,
{
    let mut targets: Vec<SocketAddr> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let address = candidate.to_canonical();
        let socket = SocketAddr::new(address, port.get());
        if targets.contains(&socket) {
            continue;
        }
        if classify(address).is_public() {
            targets.push(socket);
            continue;
        }
        let literal = Destination::new(DestinationHost::Ip(address), port);
        match admit(snapshot, agent, &literal, now) {
            Admission::Permit => targets.push(socket),
            Admission::Refuse(Fault::PolicyUnavailable) => return Err(Fault::PolicyUnavailable),
            Admission::Refuse(_) => {}
        }
    }
    if targets.is_empty() {
        return Err(Fault::AddressForbidden);
    }
    Ok(targets)
}

#[cfg(test)]
mod tests;
