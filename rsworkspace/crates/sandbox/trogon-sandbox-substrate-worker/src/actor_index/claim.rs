use trogon_sandbox_contract::LeaseId;

use super::ActorEntry;
use crate::naming::{ActorName, Atespace};

/// Records that `actor` in `atespace` is now serving `lease_id`.
///
/// Idempotent by key, so a lease retried after a partial failure overwrites its own entry
/// rather than leaving a second one behind.
pub fn claim(atespace: &Atespace, actor: &ActorName, lease_id: &LeaseId) -> ActorEntry {
    ActorEntry {
        atespace: atespace.as_str().to_string(),
        actor: actor.as_str().to_string(),
        lease_id: lease_id.clone(),
        named: actor.is_named_lease(),
        retained: false,
        owner: None,
    }
}
