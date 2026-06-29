#![cfg_attr(coverage, allow(unused_imports))]

use crate::error::SchedulerError;
use crate::projections::store::SchedulesProjectionStore;
use crate::queries::GetSchedule;
use crate::queries::read_model::Schedule;
use crate::queries::schedule_from_view;

/// Reads a single schedule from a projection backend, or `None` if absent.
#[cfg(not(coverage))]
pub async fn run(
    store: &impl SchedulesProjectionStore,
    command: GetSchedule,
) -> Result<Option<Schedule>, SchedulerError> {
    let Some(projection) = store.get_projection(&command.id).await? else {
        return Ok(None);
    };

    schedule_from_view(&projection).map(Some)
}
