use std::sync::{Arc, PoisonError, RwLock};

use crate::rule_set::RuleSet;

/// The rule set requests are evaluated against, replaceable while they run.
///
/// Cloning the handle shares it. A reload builds the new [`RuleSet`] completely
/// first, then swaps it in, so a request sees the old set or the new set and
/// never a mix. Reading is an `Arc` clone under a lock that is held for no
/// longer than that: it does no I/O and never waits on a reload in progress,
/// because the reload only takes the lock to swap.
#[derive(Debug, Clone, Default)]
pub struct BundleHandle {
    current: Arc<RwLock<Arc<RuleSet>>>,
}

impl BundleHandle {
    /// Starts serving this set.
    pub fn new(rules: RuleSet) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(rules))),
        }
    }

    /// The set in force now.
    pub fn current(&self) -> Arc<RuleSet> {
        let guard = self.current.read().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(&guard)
    }

    /// Replaces the set. Requests already running keep the set they started
    /// with.
    pub fn replace(&self, rules: RuleSet) {
        let rules = Arc::new(rules);
        let mut guard = self.current.write().unwrap_or_else(PoisonError::into_inner);
        *guard = rules;
    }
}
