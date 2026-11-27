use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, WorkerId};

/// The epoch for assigning a lease to `worker` and `attempt`, given the assignment held by `previous`. The same
/// worker and attempt keep the epoch they held (a re-send); any other pair gets the successor, so the bump belongs
/// in the write that changes the pair. A lease never assigned before starts at [`LeaseEpoch::FIRST`]. Never derived
/// from time.
pub fn lease_epoch_for_assignment(
    previous: Option<(&WorkerId, &DispatchAttemptId, Option<LeaseEpoch>)>,
    worker: &WorkerId,
    attempt: &DispatchAttemptId,
) -> LeaseEpoch {
    let Some((held_worker, held_attempt, held_epoch)) = previous else {
        return LeaseEpoch::FIRST;
    };
    if held_worker == worker && held_attempt == attempt {
        return held_epoch.unwrap_or(LeaseEpoch::FIRST);
    }
    let next = held_epoch.map_or(0, LeaseEpoch::get).saturating_add(1);
    LeaseEpoch::new(next).unwrap_or(LeaseEpoch::FIRST)
}
