use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{Digest, TenantId};

use super::listed_artifact::ListedArtifact;

/// whose record is still being written is not deleted.
#[derive(Default)]
pub struct OrphanTracker {
    first_seen: HashMap<(TenantId, Digest), SystemTime>,
}

impl OrphanTracker {
    /// Returns the orphans that have gone without a record for at least
    /// `grace`, and forgets any digest that is no longer an orphan.
    pub fn observe(&mut self, orphans: &[ListedArtifact], now: SystemTime, grace: Duration) -> Vec<ListedArtifact> {
        let current: HashSet<_> = orphans
            .iter()
            .map(|a| (a.tenant.clone(), a.reference.digest.clone()))
            .collect();
        self.first_seen.retain(|digest, _| current.contains(digest));
        let mut ready = Vec::new();
        for artifact in orphans {
            let seen = *self
                .first_seen
                .entry((artifact.tenant.clone(), artifact.reference.digest.clone()))
                .or_insert(now);
            if now.duration_since(seen).unwrap_or(Duration::ZERO) >= grace && seen < now {
                ready.push(artifact.clone());
            }
        }
        ready
    }
}
