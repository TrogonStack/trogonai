use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::load_snapshot;
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobId, JobSpec, JobStreamState, JobTransitionError, JobWriteCondition, ResolvedJobSpec,
    VersionedJobSpec, apply,
    error::CronError,
    events::{JobEvent, JobEventData},
    initial_state,
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

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
    let bucket = snapshot_bucket::run(js).await?;
    let current_snapshot = load_snapshot::<VersionedJobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, &id)
        .await
        .map_err(CronError::from)?;
    let current_state = match current_snapshot.clone() {
        Some(snapshot) => JobStreamState::try_from(snapshot).map_err(|source| {
            CronError::event_source(
                "failed to decode current job snapshot into stream state",
                source,
            )
        })?,
        None => initial_state(JobId::parse(&id).map_err(|source| {
            CronError::event_source("failed to parse job id for put job command", source)
        })?),
    };
    let event = JobEvent::job_registered(command.job().spec().clone());
    match apply(current_state, event.clone()) {
        Ok(_) => {}
        Err(JobTransitionError::CannotRegisterExistingJob { .. }) => {
            return Err(CronError::OptimisticConcurrencyConflict {
                id: id.clone(),
                expected: command.write_condition(),
                current_version: current_snapshot.as_ref().map(|job| job.version),
            });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to apply job registration to current stream state",
                error,
            ));
        }
    }

    append_events::run(js, &id, command.write_condition(), JobEventData::new(event)).await
}
