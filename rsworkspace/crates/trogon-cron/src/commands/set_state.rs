use std::fmt;

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    CronError, JobEnabledState, JobEvent, JobEventData, JobId, JobIdError, JobWriteCondition,
    SNAPSHOT_STORE_CONFIG, VersionedJobSpec, append_events, open_snapshot_bucket,
};
use trogon_eventsourcing::load_snapshot;
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
    let version = current_job_version(js, &command).await?;
    append_events(
        js,
        command.job_id.as_str(),
        JobWriteCondition::MustBeAtVersion(version),
        JobEventData::new(JobEvent::job_state_changed(
            command.job_id.to_string(),
            command.state,
        )),
    )
    .await
    .map_err(CommandError::SetState)?;
    println!("Job '{}' {}.", command.job_id, command.state.as_str());

    Ok(())
}

async fn current_job_version<J>(js: &J, command: &SetStateCommand) -> Result<u64, CommandError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    let bucket = open_snapshot_bucket(js)
        .await
        .map_err(CommandError::LoadJob)?;
    load_snapshot::<VersionedJobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, command.job_id.as_str())
        .await
        .map_err(CronError::from)
        .map_err(CommandError::LoadJob)?
        .map(|job| job.version)
        .ok_or_else(|| CommandError::JobNotFound(command.job_id.clone()))
}

impl SetStateCommand {
    pub fn new(id: String, state: JobEnabledState) -> Result<Self, CommandError> {
        Ok(Self {
            job_id: JobId::parse(&id).map_err(CommandError::InvalidJobId)?,
            state,
        })
    }
}
