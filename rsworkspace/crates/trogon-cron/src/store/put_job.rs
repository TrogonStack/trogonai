#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobDecisionError, JobId, JobSpec, JobStreamState, JobWriteCondition, ResolvedJobSpec, apply,
    error::CronError,
    events::{JobEvent, JobEventData},
    initial_state,
    nats::job_stream_state_from_snapshot,
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct PutJobCommand {
    id: JobId,
    job: ResolvedJobSpec,
    write_condition: JobWriteCondition,
}

impl PutJobCommand {
    pub fn new(spec: JobSpec, write_condition: JobWriteCondition) -> Result<Self, CronError> {
        let id = JobId::parse(&spec.id).map_err(|source| {
            CronError::event_source(
                "failed to build put job command from validated spec",
                source,
            )
        })?;
        Ok(Self {
            id,
            job: ResolvedJobSpec::try_from(&spec)?,
            write_condition,
        })
    }

    pub fn from_resolved(
        job: ResolvedJobSpec,
        write_condition: JobWriteCondition,
    ) -> Result<Self, CronError> {
        let id = JobId::parse(job.id()).map_err(|source| {
            CronError::event_source("failed to build put job command from resolved spec", source)
        })?;
        Ok(Self {
            id,
            job,
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

impl StreamCommand for PutJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<JobStreamState, JobEvent> for PutJobCommand {
    type Error = JobDecisionError;

    fn decide(state: &JobStreamState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            JobStreamState::Initial => Ok(Decision::Event(NonEmpty::one(
                JobEvent::job_registered(command.job().spec().clone()),
            ))),
            JobStreamState::Present(_) => Err(JobDecisionError::CannotRegisterExistingJob {
                id: command.stream_id().clone(),
            }),
        }
    }
}

#[cfg(not(coverage))]
pub async fn run<J>(js: &J, command: PutJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let id = command.stream_id().to_string();
    let bucket = snapshot_bucket::run(js).await?;
    let current_snapshot = load_snapshot::<JobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, &id)
        .await
        .map_err(CronError::from)?;
    let current_state = match current_snapshot.clone() {
        Some(snapshot) => job_stream_state_from_snapshot(
            command.stream_id(),
            snapshot,
            "failed to decode current job snapshot into stream state",
        )?,
        None => initial_state(),
    };

    let events = match decide(&current_state, &command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to apply job registration to current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(JobDecisionError::CannotRegisterExistingJob { .. }) => {
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
    };
    let projected_state = events
        .iter()
        .cloned()
        .try_fold(current_state, apply)
        .map_err(|error| {
            CronError::event_source(
                "failed to apply decided job registration events to current stream state",
                error,
            )
        })?;
    if !matches!(projected_state, JobStreamState::Present(_)) {
        return Err(CronError::event_source(
            "job registration decision must leave the stream present",
            std::io::Error::other(format!("job '{id}'")),
        ));
    }

    append_events::run(
        js,
        &id,
        command.write_condition(),
        events.map(JobEventData::new),
    )
    .await
}

#[cfg(coverage)]
pub async fn run<J>(_js: &J, _command: PutJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    Ok(())
}
