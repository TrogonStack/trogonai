use tracing::warn;

use super::RecoveryError;
use crate::adapter::SubstrateProvider;

/// Reclaims inactive allocations with an exact receipt owned by this worker.
pub async fn reap(provider: &SubstrateProvider, atespaces: &[String]) -> Result<usize, RecoveryError> {
    let mut reaped = 0;
    for entry in provider.index().all().await? {
        if !atespaces.contains(&entry.atespace) {
            continue;
        }
        match provider.reap_owned(&entry).await {
            Ok(true) => reaped += 1,
            Ok(false) => {}
            Err(error) => warn!(%error, actor = %entry.actor, "actor cleanup remains unconfirmed"),
        }
    }
    Ok(reaped)
}
