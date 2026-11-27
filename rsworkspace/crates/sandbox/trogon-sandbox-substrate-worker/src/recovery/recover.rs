use tracing::{info, warn};

use super::adopt::adopt;
use super::prune::prune;
use super::{RecoveryError, RecoveryReport};
use crate::actor_index::ActorUid;
use crate::adapter::SubstrateProvider;
use crate::ate_client::AteError;
use crate::proto;

/// Rebuilds the worker's active-lease map after a restart.
///
/// A restarted worker has actors it is still responsible for and no memory of them. Until
/// they are back in the active map, `run_command` for those leases matches no worker and the
/// broker times out, so this runs before the NATS subscriptions do.
pub async fn recover(provider: &SubstrateProvider) -> Result<RecoveryReport, RecoveryError> {
    let entries = provider.index().all().await?;
    let mut report = RecoveryReport::default();

    for entry in entries {
        // A retained entry is a named lease's parked state, not a live lease. Adopting it
        // would hand the worker a lease the broker does not think exists, and cancelling
        // that lease would delete the state the tenant is paying to keep.
        if entry.retained {
            report.retained += 1;
            continue;
        }
        let Some(owner) = &entry.owner else {
            warn!(actor = %entry.actor, "actor ownership is unbound; recovery requires explicit reconciliation");
            continue;
        };
        if owner.worker_id != *provider.worker_id() || owner.actor_uid.is_none() {
            continue;
        }

        let actor_ref = proto::ObjectRef {
            atespace: entry.atespace.clone(),
            name: entry.actor.clone(),
        };

        match provider.control().get_actor(actor_ref).await {
            Ok(actor) => {
                let actual_uid = actor
                    .metadata
                    .as_ref()
                    .and_then(|metadata| ActorUid::try_from(metadata.uid.clone()).ok());
                if actual_uid.is_none() || actual_uid.as_ref() != owner.actor_uid.as_ref() {
                    warn!(actor = %entry.actor, "actor resource UID is unconfirmed; preserving its ownership claim");
                    continue;
                }
                match provider.index().compare_exchange(Some(&entry), Some(&entry)).await {
                    Ok(true) => {}
                    other => {
                        warn!(result = ?other, actor = %entry.actor, "actor claim could not be confirmed during recovery");
                        continue;
                    }
                }
                if adopt(provider, &entry, &actor).await {
                    report.adopted += 1;
                } else {
                    match provider.reap_owned(&entry).await {
                        Ok(removed) => report.pruned += usize::from(removed),
                        Err(error) => {
                            warn!(%error, actor = %entry.actor, "inactive actor cleanup is unresolved; retaining its receipt")
                        }
                    }
                }
            }
            Err(AteError::NotFound { .. }) => {
                report.pruned += usize::from(prune(provider, &entry).await);
            }
            // An unreachable control plane is not evidence that an actor is gone. Leaving
            // the entry means the next pass reconsiders it; deleting it would orphan a live
            // actor with no way back.
            Err(e) => {
                warn!(error = %e, actor = %entry.actor, "could not verify actor during recovery; leaving the index entry alone");
            }
        }
    }

    info!(
        adopted = report.adopted,
        pruned = report.pruned,
        retained = report.retained,
        "substrate worker recovery complete"
    );
    Ok(report)
}
