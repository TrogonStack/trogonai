use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use super::reap;
use crate::adapter::SubstrateProvider;

/// Runs [`reap`] forever. The atespace list is a snapshot: an atespace with no actor in it
/// costs one `ListActors` per sweep, and one that appears later is picked up on the pass
/// after the first lease indexes an actor in it.
pub async fn reap_loop(provider: Arc<SubstrateProvider>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    // The first tick fires immediately, and a worker that just recovered has nothing new to
    // learn from an instant sweep.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let atespaces = match provider.index().all().await {
            Ok(entries) => {
                let mut spaces: Vec<String> = entries.into_iter().map(|entry| entry.atespace).collect();
                spaces.sort();
                spaces.dedup();
                spaces
            }
            Err(e) => {
                warn!(error = %e, "could not read the actor index; skipping this reap sweep");
                continue;
            }
        };
        if let Err(e) = reap(&provider, &atespaces).await {
            warn!(error = %e, "reap sweep failed");
        }
    }
}
