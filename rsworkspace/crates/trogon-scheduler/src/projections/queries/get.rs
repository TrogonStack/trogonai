use crate::error::SchedulerError;
use crate::projections::backend::SchedulesProjectionStore;
use crate::queries::GetSchedule;
use crate::queries::read_model::Schedule;
use crate::queries::schedule_from_view;

/// Reads a single schedule from a projection backend, or `None` if absent.
pub async fn run(
    store: &dyn SchedulesProjectionStore,
    command: GetSchedule,
) -> Result<Option<Schedule>, SchedulerError> {
    let Some(view) = store.get_view(command.id.as_str()).await? else {
        return Ok(None);
    };

    schedule_from_view(&view).map(Some)
}
