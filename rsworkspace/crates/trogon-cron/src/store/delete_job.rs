#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobDecisionError, JobId, JobSpec, JobStreamState, JobWriteCondition, apply,
    error::CronError,
    events::{JobEvent, JobEventData},
    initial_state,
    nats::job_stream_state_from_snapshot,
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct DeleteJobCommand {
    pub id: JobId,
    pub write_condition: JobWriteCondition,
}

impl StreamCommand for DeleteJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<JobStreamState, JobEvent> for DeleteJobCommand {
    type Error = JobDecisionError;

    fn decide(state: &JobStreamState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            JobStreamState::Initial => Err(JobDecisionError::MissingJobForRemoval {
                id: command.stream_id().clone(),
            }),
            JobStreamState::Present(_) => Ok(Decision::Event(NonEmpty::one(
                JobEvent::job_removed(command.stream_id().to_string()),
            ))),
        }
    }
}

#[cfg(not(coverage))]
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
    let current_snapshot = load_snapshot::<JobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, id.as_str())
        .await
        .map_err(CronError::from)?;
    let current_state = match current_snapshot.clone() {
        Some(snapshot) => job_stream_state_from_snapshot(
            &id,
            snapshot,
            "failed to decode current job snapshot into stream state",
        )?,
        None => initial_state(),
    };
    let events = match decide(
        &current_state,
        &DeleteJobCommand {
            id: id.clone(),
            write_condition,
        },
    ) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to decide job removal from current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(JobDecisionError::MissingJobForRemoval { .. }) => {
            return Err(CronError::JobNotFound { id: id.to_string() });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to decide job removal from current stream state",
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
                "failed to apply decided job removal events to current stream state",
                error,
            )
        })?;
    if !matches!(projected_state, JobStreamState::Initial) {
        return Err(CronError::event_source(
            "job removal decision must leave the stream initial",
            std::io::Error::other(format!("job '{}'", id)),
        ));
    }

    append_events::run(
        js,
        id.as_str(),
        write_condition,
        events.map(JobEventData::new),
    )
    .await
}

#[cfg(coverage)]
pub async fn run<J>(_js: &J, _command: DeleteJobCommand) -> Result<(), CronError>
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
