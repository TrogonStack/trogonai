use crate::error::SchedulerError;
use crate::projections::PostgresSchedulesProjection;
use crate::queries::GetSchedule;
use crate::queries::decode::schedule_from_view;
use crate::queries::read_model::Schedule;

/// Reads a single schedule from the Postgres projection, or `None` if absent.
pub async fn run(
    store: &PostgresSchedulesProjection,
    command: GetSchedule,
) -> Result<Option<Schedule>, SchedulerError> {
    let Some(projection) = store.get_projection(&command.id).await? else {
        return Ok(None);
    };

    schedule_from_view(&projection).map(Some)
}
