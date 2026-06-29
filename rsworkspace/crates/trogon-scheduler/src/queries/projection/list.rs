#![cfg_attr(coverage, allow(unused_imports))]

use crate::error::SchedulerError;
use crate::projections::store::SchedulesProjectionStore;
use crate::queries::ListSchedules;
use crate::queries::read_model::Schedule;
use crate::queries::schedule_from_view;

/// Lists every schedule from a projection backend.
#[cfg(not(coverage))]
pub async fn run(
    store: &dyn SchedulesProjectionStore,
    _command: ListSchedules,
) -> Result<Vec<Schedule>, SchedulerError> {
    let views = store.list_views().await?;
    let mut schedules = Vec::with_capacity(views.len());
    for view in &views {
        // One malformed view must not suppress every other schedule in the listing.
        // Skip it with a warning; catch-up rebuilds the read model on restart.
        match schedule_from_view(view) {
            Ok(schedule) => schedules.push(schedule),
            Err(source) => {
                tracing::warn!(schedule_id = %view.schedule_id, %source, "skipping unreadable projected schedule during list");
            }
        }
    }

    Ok(schedules)
}
