use tracing::warn;

use super::atespace_of::atespace_of;
use crate::actor_index::ActorEntry;
use crate::adapter::{ActiveLease, SubstrateProvider};
use crate::naming::ActorName;
use crate::proto::{self, ActorState};

/// Puts one entry back into the active map, unless the actor cannot serve it.
pub(super) async fn adopt(provider: &SubstrateProvider, entry: &ActorEntry, actor: &proto::Actor) -> bool {
    // A crashed or vanishing actor cannot run a command, and a suspended one is not a live
    // lease (a live lease's actor is resumed for the whole lease). Reporting the lease as
    // held would make every later command fail against an actor that will never answer.
    let state = actor.status.as_ref().map(|status| status.state()).unwrap_or_default();
    if !matches!(state, ActorState::Running | ActorState::Resuming) {
        return false;
    }

    let Some(name) = ActorName::from_existing(&entry.actor) else {
        warn!(actor = %entry.actor, "index entry names an actor outside this worker's naming scheme");
        return false;
    };
    let Some(atespace) = atespace_of(&entry.atespace) else {
        warn!(atespace = %entry.atespace, "index entry names an atespace this worker cannot address");
        return false;
    };

    if let Err(error) = provider
        .adopt(
            entry.lease_id.clone(),
            ActiveLease {
                atespace,
                actor: name,
                named: entry.named,
                // Recovery cannot see the original request, and a named actor that survived
                // a restart is by definition one worth keeping. Guessing `false` here would
                // let a later cancel delete a tenant's persisted state.
                persistent: entry.named,
                // The port the lease exposed is not recorded anywhere the worker can read
                // back, so a recovered lease reports no exposed port until it is re-leased.
                // Under-reporting produces a `PortNotExposed`, which is recoverable;
                // over-reporting would publish an endpoint that routes nowhere.
                exposed_port: None,
            },
            entry.clone(),
            true,
        )
        .await
    {
        warn!(%error, actor = %entry.actor, "could not adopt the actor during recovery");
        return false;
    }
    true
}
