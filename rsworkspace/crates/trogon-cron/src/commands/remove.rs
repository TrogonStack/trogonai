use std::fmt;

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    DeleteJobCommand, GetJobCommand as StoreGetJobCommand, JobId, JobIdError, JobWriteCondition,
    delete_job, get_job,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct RemoveCommand {
    pub job_id: JobId,
}

#[derive(Debug)]
pub enum CommandError {
    InvalidJobId(JobIdError),
    GetJob(trogon_cron::CronError),
    JobNotFound(JobId),
    DeleteJob(trogon_cron::CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::GetJob(source) => write!(f, "failed to load job: {source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
            Self::DeleteJob(source) => write!(f, "failed to remove job: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJobId(source) => Some(source),
            Self::GetJob(source) => Some(source),
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
    let version = current_job_version(js, &command).await?;
    delete_job(
        js,
        DeleteJobCommand {
            id: command.job_id.clone(),
            write_condition: JobWriteCondition::MustBeAtVersion(version),
        },
    )
        .await
        .map_err(CommandError::DeleteJob)?;
    println!("Job '{}' removed.", command.job_id);

    Ok(())
}

async fn current_job_version<J>(js: &J, command: &RemoveCommand) -> Result<u64, CommandError>
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

impl TryFrom<String> for RemoveCommand {
    type Error = CommandError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Ok(Self {
            job_id: JobId::parse(&id).map_err(CommandError::InvalidJobId)?,
        })
    }
}
