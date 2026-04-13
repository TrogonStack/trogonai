use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::load_snapshot;
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId, JobStreamState, JobTransitionError, JobWriteCondition, VersionedJobSpec, apply,
    error::CronError,
    events::{JobEvent, JobEventData},
    initial_state,
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct DeleteJobCommand {
    pub id: JobId,
    pub write_condition: JobWriteCondition,
}

pub async fn run<J>(js: &J, command: DeleteJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let DeleteJobCommand {
        id,
        write_condition,
    } = command;

    let bucket = snapshot_bucket::run(js).await?;
    let current_snapshot =
        load_snapshot::<VersionedJobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, id.as_str())
            .await
            .map_err(CronError::from)?;
    let current_state = match current_snapshot.clone() {
        Some(snapshot) => JobStreamState::try_from(snapshot).map_err(|source| {
            CronError::event_source(
                "failed to decode current job snapshot into stream state",
                source,
            )
        })?,
        None => initial_state(id.clone()),
    };
    let event = JobEvent::job_removed(id.to_string());
    match apply(current_state, event.clone()) {
        Ok(_) => {}
        Err(JobTransitionError::MissingJobForRemoval { .. }) => {
            return Err(CronError::JobNotFound { id: id.to_string() });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to apply job removal to current stream state",
                error,
            ));
        }
    }
    append_events::run(js, id.as_str(), write_condition, JobEventData::new(event)).await
}
