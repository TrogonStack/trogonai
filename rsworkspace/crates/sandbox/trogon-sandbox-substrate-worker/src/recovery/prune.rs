use tracing::warn;

use crate::actor_index::ActorEntry;
use crate::adapter::SubstrateProvider;

pub(super) async fn prune(provider: &SubstrateProvider, entry: &ActorEntry) -> bool {
    match provider.index().compare_exchange(Some(entry), None).await {
        Ok(pruned) => pruned,
        Err(error) => {
            warn!(%error, actor = %entry.actor, "failed to prune a dead index entry");
            false
        }
    }
}
