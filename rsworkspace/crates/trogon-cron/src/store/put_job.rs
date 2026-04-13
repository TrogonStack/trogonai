use async_nats::jetstream::{self, context, kv};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobSpec, JobWriteCondition, ResolvedJobSpec,
    error::CronError,
    events::{JobEvent, JobEventData},
};

use super::append_events;

#[derive(Debug, Clone)]
pub struct PutJobCommand {
    job: ResolvedJobSpec,
    write_condition: JobWriteCondition,
}

impl PutJobCommand {
    pub fn new(spec: JobSpec, write_condition: JobWriteCondition) -> Result<Self, CronError> {
        Ok(Self {
            job: ResolvedJobSpec::try_from(&spec)?,
            write_condition,
        })
    }

    pub fn job(&self) -> &ResolvedJobSpec {
        &self.job
    }

    pub const fn write_condition(&self) -> JobWriteCondition {
        self.write_condition
    }
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
    let id = command.job().id().to_string();
    append_events::run(
        js,
        &id,
        command.write_condition(),
        JobEventData::new(JobEvent::job_registered(command.job().spec().clone())),
    )
    .await
}
