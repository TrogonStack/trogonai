use std::fmt;

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    CronError, JobDecisionError, JobEnabledState, JobId, JobIdError, JobStreamState,
    JobWriteCondition, SNAPSHOT_STORE_CONFIG, SetJobStateCommand as StoreSetJobStateCommand,
    VersionedJobSpec, append_events, initial_state, open_snapshot_bucket,
};
use trogon_eventsourcing::{Decision, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct SetStateCommand {
    pub job_id: JobId,
    pub state: JobEnabledState,
}

#[derive(Debug)]
pub enum CommandError {
    InvalidJobId(JobIdError),
    LoadJob(CronError),
    JobNotFound(JobId),
    SetState(CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::LoadJob(source) => write!(f, "failed to load job: {source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
            Self::SetState(source) => write!(f, "failed to update job state: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJobId(source) => Some(source),
            Self::LoadJob(source) => Some(source),
            Self::JobNotFound(_) => None,
            Self::SetState(source) => Some(source),
        }
    }
}

pub async fn run<J>(js: &J, command: SetStateCommand) -> Result<(), CommandError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let bucket = open_snapshot_bucket(js)
        .await
        .map_err(CommandError::LoadJob)?;
    let current_snapshot =
        load_snapshot::<VersionedJobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, command.job_id.as_str())
            .await
            .map_err(CronError::from)
            .map_err(CommandError::LoadJob)?;
    let current_state = match current_snapshot.clone() {
        Some(snapshot) => JobStreamState::try_from(snapshot).map_err(|source| {
            CommandError::LoadJob(CronError::event_source(
                "failed to decode current job snapshot into stream state",
                source,
            ))
        })?,
        None => initial_state(),
    };
    let write_condition = current_snapshot
        .as_ref()
        .map(|job| JobWriteCondition::MustBeAtVersion(job.version))
        .unwrap_or(JobWriteCondition::MustNotExist);
    let store_command = StoreSetJobStateCommand {
        id: command.job_id.clone(),
        state: command.state,
        write_condition,
    };
    let events = match decide(&current_state, &store_command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CommandError::SetState(CronError::event_source(
                "failed to decide job state change from current stream state",
                std::io::Error::other("unsupported decision variant"),
            )));
        }
        Err(JobDecisionError::MissingJobForStateChange { .. }) => {
            return Err(CommandError::JobNotFound(command.job_id.clone()));
        }
        Err(JobDecisionError::StateAlreadySet { state, .. }) => {
            return Err(CommandError::SetState(CronError::JobStateAlreadySet {
                id: command.job_id.to_string(),
                state,
            }));
        }
        Err(error) => {
            return Err(CommandError::SetState(CronError::event_source(
                "failed to decide job state change from current stream state",
                error,
            )));
        }
    };
    append_events(
        js,
        command.job_id.as_str(),
        write_condition,
        events.map(trogon_cron::JobEventData::new),
    )
    .await
    .map_err(CommandError::SetState)?;
    println!("Job '{}' {}.", command.job_id, command.state.as_str());

    Ok(())
}
impl SetStateCommand {
    pub fn new(id: String, state: JobEnabledState) -> Result<Self, CommandError> {
        Ok(Self {
            job_id: JobId::parse(&id).map_err(CommandError::InvalidJobId)?,
            state,
        })
    }
}
