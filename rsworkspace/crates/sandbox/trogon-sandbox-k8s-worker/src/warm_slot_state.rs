use std::collections::BTreeMap;

use trogon_sandbox_providers::{LABEL_WARM_POOL, WARM_POOL_CLAIMED, WARM_POOL_IDLE};

use crate::constants::{POD_PHASE_FAILED, POD_PHASE_RUNNING, POD_PHASE_SUCCEEDED};

/// Where a warm pod stands in its life, read from the pod alone so a restarted
/// worker agrees with the one that came before it.
///
/// `Warming -> Ready -> Claimed -> Terminating` is the whole path. The claim
/// and the bind to a lease are one fenced label write, so a pod is never
/// half-claimed; what a crash can leave is a `Claimed` pod, which the lease
/// recovery and the claimed-pod sweep already own, and a `Failed` one, which
/// the reconcile pass deletes.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WarmSlotState {
    /// Idle and still starting.
    Warming,
    /// Idle and running: a claim can take it.
    Ready,
    /// Bound to a lease.
    Claimed,
    Terminating,
    /// Idle but terminal (failed or completed); it will never serve a lease.
    Failed,
}

impl WarmSlotState {
    /// The slot state of a pod with these `labels`, `phase` and deletion mark, or
    /// `None` when it is not a warm pool pod.
    pub fn of(labels: Option<&BTreeMap<String, String>>, phase: Option<&str>, deleting: bool) -> Option<WarmSlotState> {
        let pool_label = labels?.get(LABEL_WARM_POOL)?.as_str();
        if deleting {
            return Some(WarmSlotState::Terminating);
        }
        match pool_label {
            WARM_POOL_CLAIMED => Some(WarmSlotState::Claimed),
            WARM_POOL_IDLE => Some(match phase {
                Some(POD_PHASE_RUNNING) => WarmSlotState::Ready,
                Some(POD_PHASE_FAILED | POD_PHASE_SUCCEEDED) => WarmSlotState::Failed,
                _ => WarmSlotState::Warming,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
