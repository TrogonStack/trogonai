use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::LeaseId;

use super::{ActorOwner, entry_key};
use crate::naming::ActorName;

/// What the index records about one actor.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActorEntry {
    pub atespace: String,
    pub actor: String,
    /// The lease currently served by this actor. For a named actor this changes on every
    /// reconnect, which is the whole reason the entry exists.
    pub lease_id: LeaseId,
    /// Whether the actor backs a named lease, and so may outlive the lease it serves.
    pub named: bool,
    /// Set when a named lease was cancelled but its actor was kept suspended so a later
    /// reconnect finds the tenant's state. A retained entry has no live lease: recovery must
    /// not put it back in the active map, and the reaper must not delete it.
    #[serde(default)]
    pub retained: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ActorOwner>,
}

impl ActorEntry {
    pub fn key(&self) -> String {
        entry_key(&self.atespace, &self.actor)
    }

    pub fn actor_name(&self) -> Option<ActorName> {
        ActorName::from_existing(&self.actor)
    }
}
