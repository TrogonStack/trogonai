use std::fmt;

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    GetJobCommand as StoreGetJobCommand, JobEnabledState, JobId, JobIdError, JobWriteCondition,
    SetJobStateCommand as StoreSetJobStateCommand, get_job, set_job_state,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct SetStateCommand {
    pub job_id: JobId,
    pub state: JobEnabledState,
}

#[derive(Debug)]
pub enum CommandError {
    InvalidJobId(JobIdError),
    GetJob(trogon_cron::CronError),
    JobNotFound(JobId),
    SetState(trogon_cron::CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::GetJob(source) => write!(f, "failed to load job: {source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
            Self::SetState(source) => write!(f, "failed to update job state: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJobId(source) => Some(source),
            Self::GetJob(source) => Some(source),
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
    set_job_state(
        js,
        StoreSetJobStateCommand {
            id: command.job_id.clone(),
            state: command.state,
            write_condition: JobWriteCondition::MustBeAtVersion(version),
        },
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
    get_job(
        js,
        StoreGetJobCommand {
            id: command.job_id.clone(),
        },
    )
        .await
        .map_err(CommandError::GetJob)?
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
