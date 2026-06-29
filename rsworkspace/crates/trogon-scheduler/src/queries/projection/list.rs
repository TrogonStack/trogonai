#![cfg_attr(coverage, allow(unused_imports))]

use crate::error::SchedulerError;
use crate::projections::store::SchedulesProjectionStore;
use crate::queries::ListSchedules;
use crate::queries::read_model::Schedule;
use crate::queries::schedule_from_view;

/// Lists every schedule from a projection backend.
#[cfg(not(coverage))]
pub async fn run(
    store: &impl SchedulesProjectionStore,
    _command: ListSchedules,
) -> Result<Vec<Schedule>, SchedulerError> {
    let projections = store.list_projections().await?;
    let mut schedules = Vec::with_capacity(projections.len());
    for projection in &projections {
        // One malformed projection must not suppress every other schedule in the
        // listing. Skip it with a warning; catch-up rebuilds the read model on restart.
        match schedule_from_view(projection) {
            Ok(schedule) => schedules.push(schedule),
            Err(source) => {
                tracing::warn!(schedule_id = %projection.schedule_id, %source, "skipping unreadable projected schedule during list");
            }
        }
    }

    Ok(schedules)
}
