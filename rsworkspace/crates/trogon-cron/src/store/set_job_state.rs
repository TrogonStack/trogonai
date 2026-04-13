#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobDecisionError, JobEnabledState, JobId, JobStreamState, JobWriteCondition, VersionedJobSpec,
    apply,
    error::CronError,
    events::{JobEvent, JobEventData},
    initial_state,
};

use super::{SNAPSHOT_STORE_CONFIG, append_events, snapshot_bucket};

#[derive(Debug, Clone)]
pub struct SetJobStateCommand {
    pub id: JobId,
    pub state: JobEnabledState,
    pub write_condition: JobWriteCondition,
}

impl StreamCommand for SetJobStateCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<JobStreamState, JobEvent> for SetJobStateCommand {
    type Error = JobDecisionError;

    fn decide(state: &JobStreamState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            JobStreamState::Initial => Err(JobDecisionError::MissingJobForStateChange {
                id: command.stream_id().clone(),
            }),
            JobStreamState::Present(spec) if spec.state == command.state => {
                Err(JobDecisionError::StateAlreadySet {
                    id: command.stream_id().clone(),
                    state: command.state,
                })
            }
            JobStreamState::Present(_) => Ok(Decision::Event(NonEmpty::one(
                JobEvent::job_state_changed(command.stream_id().to_string(), command.state),
            ))),
        }
    }
}

#[cfg(not(coverage))]
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
        None => initial_state(),
    };
    let events = match decide(
        &current_state,
        &SetJobStateCommand {
            id: id.clone(),
            state,
            write_condition,
        },
    ) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to decide job state change from current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(JobDecisionError::MissingJobForStateChange { .. }) => {
            return Err(CronError::JobNotFound { id: id.to_string() });
        }
        Err(JobDecisionError::StateAlreadySet { state, .. }) => {
            return Err(CronError::JobStateAlreadySet {
                id: id.to_string(),
                state,
            });
        }
        Err(error) => {
            return Err(CronError::event_source(
                "failed to decide job state change from current stream state",
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
                "failed to apply decided job state events to current stream state",
                error,
            )
        })?;
    match projected_state {
        JobStreamState::Present(spec) if spec.state == state => {}
        _ => {
            return Err(CronError::event_source(
                "job state decision must leave the stream present at the target state",
                std::io::Error::other(format!("job '{}'", id)),
            ));
        }
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
pub async fn run<J>(_js: &J, _command: SetJobStateCommand) -> Result<(), CronError>
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
