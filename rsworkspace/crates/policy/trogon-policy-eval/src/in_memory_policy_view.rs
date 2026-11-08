use std::sync::{Arc, PoisonError, RwLock};

use crate::snapshot::PolicySnapshot;
use crate::view::PolicyView;

/// A view over a snapshot held in memory, for fixtures, tests, and sources
/// that load a bundle once.
///
/// A bundle source is a bounded-stale source like any other: the snapshot's
/// `as_of` must show when its producer last vouched for it, and the evaluator
/// refuses once that is past the budget (ADR#0071 Decision 7).
#[derive(Debug)]
pub struct InMemoryPolicyView {
    current: RwLock<Arc<PolicySnapshot>>,
}

impl InMemoryPolicyView {
    /// Starts a view at this snapshot.
    pub fn new(snapshot: PolicySnapshot) -> Self {
        Self {
            current: RwLock::new(Arc::new(snapshot)),
        }
    }

    /// Replaces the snapshot. Evaluations already holding the old one finish
    /// against it.
    pub fn replace(&self, snapshot: PolicySnapshot) {
        // A poisoned lock only means another thread panicked while holding it.
        // The value is an `Arc` that is replaced whole, so it is never torn.
        let mut current = self.current.write().unwrap_or_else(PoisonError::into_inner);
        *current = Arc::new(snapshot);
    }
}

impl PolicyView for InMemoryPolicyView {
    fn snapshot(&self) -> Arc<PolicySnapshot> {
        let current = self.current.read().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(&current)
    }
}
