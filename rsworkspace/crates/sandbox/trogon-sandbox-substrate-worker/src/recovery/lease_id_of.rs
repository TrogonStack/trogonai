use trogon_sandbox_contract::LeaseId;

use crate::naming::ActorName;

/// The lease id an anonymous actor's name encodes, for logging a reap.
pub fn lease_id_of(actor: &str) -> Option<LeaseId> {
    ActorName::from_existing(actor).and_then(|name| name.lease_id())
}
