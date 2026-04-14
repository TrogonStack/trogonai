use std::{fmt, io::Read};

use async_nats::jetstream::{self, context, kv};
use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::{
    JobDecisionError, JobId, JobSpec, JobWriteCondition, ResolvedJobSpec,
    error::CronError,
    events::{JobEvent, JobEventData},
    store::{SNAPSHOT_STORE_CONFIG, append_events, open_snapshot_bucket},
};

#[derive(Debug, Clone)]
pub struct RegisterJobCommand {
    id: JobId,
    job: ResolvedJobSpec,
    write_condition: JobWriteCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterJobState {
    Missing,
    Present,
}

#[derive(Debug)]
pub enum ReadRegisterJobCommandError {
    ReadStdin(std::io::Error),
    DeserializeJobSpec(serde_json::Error),
    InvalidJobSpec(CronError),
}

impl fmt::Display for ReadRegisterJobCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadStdin(source) => write!(f, "failed to read stdin: {source}"),
            Self::DeserializeJobSpec(source) => write!(f, "invalid job spec payload: {source}"),
            Self::InvalidJobSpec(source) => write!(f, "invalid job spec: {source}"),
        }
    }
}

impl std::error::Error for ReadRegisterJobCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadStdin(source) => Some(source),
            Self::DeserializeJobSpec(source) => Some(source),
            Self::InvalidJobSpec(source) => Some(source),
        }
    }
}

impl RegisterJobCommand {
    pub fn new(spec: JobSpec, write_condition: JobWriteCondition) -> Result<Self, CronError> {
        let id = JobId::parse(&spec.id).map_err(|source| {
            CronError::event_source(
                "failed to build register job command from validated spec",
                source,
            )
        })?;
        Ok(Self {
            id,
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

    pub fn state_from_snapshot(
        &self,
        snapshot: Option<&trogon_eventsourcing::Snapshot<JobSpec>>,
    ) -> Result<RegisterJobState, CronError> {
        match snapshot {
            None => Ok(RegisterJobState::Missing),
            Some(snapshot) if snapshot.payload.id == self.stream_id().as_str() => {
                Ok(RegisterJobState::Present)
            }
            Some(snapshot) => Err(CronError::event_source(
                "failed to decode current job snapshot into register-job state",
                std::io::Error::other(format!(
                    "expected '{}' but snapshot carried '{}'",
                    self.stream_id(),
                    snapshot.payload.id
                )),
            )),
        }
    }
}

impl StreamCommand for RegisterJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<RegisterJobState, JobEvent> for RegisterJobCommand {
    type Error = JobDecisionError;

    fn decide(state: &RegisterJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            RegisterJobState::Missing => Ok(Decision::Event(NonEmpty::one(
                JobEvent::job_registered(command.job().spec().clone()),
            ))),
            RegisterJobState::Present => Err(JobDecisionError::CannotRegisterExistingJob {
                id: command.stream_id().clone(),
            }),
        }
    }
}

pub fn read_from_stdin() -> Result<RegisterJobCommand, ReadRegisterJobCommandError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(ReadRegisterJobCommandError::ReadStdin)?;

    let spec =
        serde_json::from_str(&buf).map_err(ReadRegisterJobCommandError::DeserializeJobSpec)?;
    RegisterJobCommand::new(spec, JobWriteCondition::MustNotExist)
        .map_err(ReadRegisterJobCommandError::InvalidJobSpec)
}

#[cfg(not(coverage))]
pub async fn run<J>(js: &J, command: RegisterJobCommand) -> Result<(), CronError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let id = command.stream_id().to_string();
    let bucket = open_snapshot_bucket(js).await?;
    let current_snapshot = load_snapshot::<JobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, &id)
        .await
        .map_err(CronError::from)?;
    let current_state = command.state_from_snapshot(current_snapshot.as_ref())?;

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
                "failed to decide job registration from current register-job state",
                error,
            ));
        }
    };

    append_events(
        js,
        &id,
        command.write_condition(),
        events.map(JobEventData::new),
    )
    .await
}

#[cfg(coverage)]
pub async fn run<J>(_js: &J, _command: RegisterJobCommand) -> Result<(), CronError>
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
