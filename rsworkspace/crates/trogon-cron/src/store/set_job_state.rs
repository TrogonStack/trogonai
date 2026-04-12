use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::load_snapshot;
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobEnabledState, JobId, JobWriteCondition,
    VersionedJobSpec,
    error::CronError,
    events::{JobEvent, JobEventData},
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct SetJobStateCommand {
    pub id: JobId,
    pub state: JobEnabledState,
    pub write_condition: JobWriteCondition,
}

pub async fn run<J>(js: &J, command: SetJobStateCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let SetJobStateCommand {
        id,
        state,
        write_condition,
    } = command;

    let bucket = snapshot_bucket::run(js).await?;
    load_snapshot::<VersionedJobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, id.as_str())
        .await
        .map_err(CronError::from)?
        .ok_or_else(|| CronError::JobNotFound { id: id.to_string() })?;
    append_events::run(
        js,
        id.as_str(),
        write_condition,
        JobEventData::new(JobEvent::job_state_changed(id.to_string(), state)),
    )
    .await
}
