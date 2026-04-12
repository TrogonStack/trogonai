use std::fmt;

use async_nats::jetstream::kv;
use trogon_cron::{GetJobCommand as StoreGetJobCommand, JobId, JobIdError, get_job};
use trogon_nats::jetstream::JetStreamGetKeyValue;

#[derive(Debug)]
pub struct GetCommand {
    pub job_id: JobId,
}

#[derive(Debug)]
pub enum CommandError {
    GetJob(trogon_cron::CronError),
    SerializeJob(serde_json::Error),
    InvalidJobId(JobIdError),
    JobNotFound(JobId),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GetJob(source) => write!(f, "failed to load job: {source}"),
            Self::SerializeJob(source) => write!(f, "failed to serialize job: {source}"),
            Self::InvalidJobId(source) => write!(f, "{source}"),
            Self::JobNotFound(id) => write!(f, "job '{id}' not found"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GetJob(source) => Some(source),
            Self::SerializeJob(source) => Some(source),
            Self::InvalidJobId(source) => Some(source),
            Self::JobNotFound(_) => None,
        }
    }
}

pub async fn run<J>(js: &J, command: GetCommand) -> Result<(), CommandError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>,
{
    match get_job(
        js,
        StoreGetJobCommand {
            id: command.job_id.clone(),
        },
    )
        .await
        .map_err(CommandError::GetJob)?
    {
        Some(job) => println!(
            "{}",
            serde_json::to_string_pretty(&job).map_err(CommandError::SerializeJob)?
        ),
        None => return Err(CommandError::JobNotFound(command.job_id)),
    }

    Ok(())
}

impl TryFrom<String> for GetCommand {
    type Error = CommandError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Ok(Self {
            job_id: JobId::parse(&id).map_err(CommandError::InvalidJobId)?,
        })
    }
}
