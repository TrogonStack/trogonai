use async_nats::jetstream::{self, context, kv};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobSpec, JobWriteCondition,
    domain::validate_job_spec,
    error::CronError,
    events::{JobEvent, JobEventData},
};

use super::append_events;

#[derive(Debug, Clone)]
pub struct PutJobCommand {
    pub spec: JobSpec,
    pub write_condition: JobWriteCondition,
}

pub async fn run<J>(js: &J, command: PutJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let PutJobCommand {
        spec,
        write_condition,
    } = command;
    let id = spec.id.clone();

    validate_job_spec(&spec)?;
    append_events::run(
        js,
        &id,
        write_condition,
        JobEventData::new(JobEvent::job_registered(spec)),
    )
    .await
}
