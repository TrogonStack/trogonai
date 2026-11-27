//! Which actor is serving which lease.
//!
//! The worker's active-lease map cannot be rebuilt from `ListActors` alone. That works only
//! while every actor name encodes its lease id, and named leases break exactly that: the
//! broker mints a fresh `LeaseId` each time a named lease is re-established, so the actor
//! holding the tenant's state cannot be named after any one lease without being stranded on
//! the next reconnect (see [`crate::naming::ActorName::for_named_lease`]). The name therefore
//! maps only one way, and the reverse direction has to be recorded somewhere.
//!
//! It also gives the reaper a safe rule. An actor with a `sbx-`/`sbxn-` prefix and no index
//! entry is unclaimed, and unclaimed is the only thing the reaper deletes.

mod actor_claim_id;
mod actor_claim_id_error;
mod actor_entry;
mod actor_index_error;
mod actor_index_result;
mod actor_owner;
mod actor_uid;
mod actor_uid_error;
mod claim;
mod constants;
mod entry_codec;
mod entry_key;
mod exchange_key;
mod index;
#[cfg(test)]
pub(crate) mod memory;
mod nats_actor_index;

pub use actor_claim_id::ActorClaimId;
pub use actor_claim_id_error::ActorClaimIdError;
pub use actor_entry::ActorEntry;
pub use actor_index_error::ActorIndexError;
pub use actor_index_result::ActorIndexResult;
pub use actor_owner::ActorOwner;
pub use actor_uid::ActorUid;
pub use actor_uid_error::ActorUidError;
pub use claim::claim;
pub use constants::ACTOR_KV_BUCKET;
pub use entry_key::entry_key;
pub use index::ActorIndex;
pub use nats_actor_index::NatsActorIndex;

#[cfg(test)]
mod tests;
