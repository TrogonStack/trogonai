use std::fmt;

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    CronError, DeleteJobCommand, JobDecisionError, JobId, JobIdError, JobStreamState,
    JobWriteCondition, SNAPSHOT_STORE_CONFIG, VersionedJobSpec, append_events, initial_state,
    open_snapshot_bucket,
};
use trogon_eventsourcing::{Decision, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct RemoveCommand {
    pub job_id: JobId,
}

#[derive(Debug)]
pub enum CommandError {
    InvalidJobId(JobIdError),
    LoadJob(CronError),
    JobNotFound(JobId),
    DeleteJob(CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::LoadJob(source) => write!(f, "failed to load job: {source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
            Self::DeleteJob(source) => write!(f, "failed to remove job: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJobId(source) => Some(source),
            Self::LoadJob(source) => Some(source),
            Self::JobNotFound(_) => None,
            Self::DeleteJob(source) => Some(source),
        }
    }
}

pub async fn run<J>(js: &J, command: RemoveCommand) -> Result<(), CommandError>
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
        None => initial_state(command.job_id.clone()),
    };
    let write_condition = current_snapshot
        .as_ref()
        .map(|job| JobWriteCondition::MustBeAtVersion(job.version))
        .unwrap_or(JobWriteCondition::MustNotExist);
    let store_command = DeleteJobCommand {
        id: command.job_id.clone(),
        write_condition,
    };
    let events = match decide(&current_state, &store_command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CommandError::DeleteJob(CronError::event_source(
                "failed to decide job removal from current stream state",
                std::io::Error::other("unsupported decision variant"),
            )));
        }
        Err(JobDecisionError::MissingJobForRemoval { .. }) => {
            return Err(CommandError::JobNotFound(command.job_id.clone()));
        }
        Err(error) => {
            return Err(CommandError::DeleteJob(CronError::event_source(
                "failed to decide job removal from current stream state",
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
    .map_err(CommandError::DeleteJob)?;
    println!("Job '{}' removed.", command.job_id);

    Ok(())
}
impl TryFrom<String> for RemoveCommand {
    type Error = CommandError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Ok(Self {
            job_id: JobId::parse(&id).map_err(CommandError::InvalidJobId)?,
        })
    }
}
