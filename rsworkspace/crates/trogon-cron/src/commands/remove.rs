use std::fmt;

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    CronError, JobEvent, JobEventData, JobId, JobIdError, JobStreamState, JobTransitionError,
    JobWriteCondition, SNAPSHOT_STORE_CONFIG, VersionedJobSpec, append_events, apply,
    initial_state, open_snapshot_bucket,
};
use trogon_eventsourcing::load_snapshot;
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
    let event = JobEvent::job_removed(command.job_id.to_string());
    match apply(current_state, event.clone()) {
        Ok(_) => {}
        Err(JobTransitionError::MissingJobForRemoval { .. }) => {
            return Err(CommandError::JobNotFound(command.job_id.clone()));
        }
        Err(error) => {
            return Err(CommandError::DeleteJob(CronError::event_source(
                "failed to apply job removal to current stream state",
                error,
            )));
        }
    }
    let version = current_snapshot
        .as_ref()
        .map(|job| job.version)
        .ok_or_else(|| CommandError::JobNotFound(command.job_id.clone()))?;
    append_events(
        js,
        command.job_id.as_str(),
        JobWriteCondition::MustBeAtVersion(version),
        JobEventData::new(event),
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
