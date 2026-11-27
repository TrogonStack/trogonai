use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, WorkerId};

/// The epoch for assigning `worker` and `attempt` to a lease whose current assignment is
/// `previous`. The same pair keeps its epoch (a re-send); any other pair gets the
/// successor, so the bump belongs in the write that changes the pair. Never derived
/// from time.
pub(crate) fn epoch_for_assignment(
    previous: Option<(&WorkerId, &DispatchAttemptId, Option<LeaseEpoch>)>,
    worker: &WorkerId,
    attempt: &DispatchAttemptId,
) -> LeaseEpoch {
    match previous {
        None => LeaseEpoch::FIRST,
        Some((prior_worker, prior_attempt, epoch)) => {
            let current = epoch.map_or(0, LeaseEpoch::get);
            if prior_worker == worker && prior_attempt == attempt {
                epoch.map_or(LeaseEpoch::FIRST, |epoch| epoch.max(LeaseEpoch::FIRST))
            } else {
                LeaseEpoch::new(current.saturating_add(1)).unwrap_or(LeaseEpoch::FIRST)
            }
        }
    }
}
