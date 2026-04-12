use std::{fmt, io::Read};

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    CronError, JobEvent, JobEventData, JobSpec, JobWriteCondition, append_events, validate_job_spec,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct AddCommand {
    pub spec: JobSpec,
}

#[derive(Debug)]
pub enum CommandError {
    ReadStdin(std::io::Error),
    InvalidJobSpec(serde_json::Error),
    RegisterJob(CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadStdin(source) => write!(f, "failed to read stdin: {source}"),
            Self::InvalidJobSpec(source) => write!(f, "invalid job spec: {source}"),
            Self::RegisterJob(source) => write!(f, "failed to register job: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadStdin(source) => Some(source),
            Self::InvalidJobSpec(source) => Some(source),
            Self::RegisterJob(source) => Some(source),
        }
    }
}

pub fn read_from_stdin() -> Result<AddCommand, CommandError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(CommandError::ReadStdin)?;

    let spec = serde_json::from_str(&buf).map_err(CommandError::InvalidJobSpec)?;
    Ok(AddCommand { spec })
}

pub async fn run<J>(js: &J, command: AddCommand) -> Result<(), CommandError>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    let id = command.spec.id.clone();
    validate_job_spec(&command.spec).map_err(CommandError::RegisterJob)?;
    append_events(
        js,
        &id,
        JobWriteCondition::MustNotExist,
        JobEventData::new(JobEvent::job_registered(command.spec)),
    )
    .await
    .map_err(CommandError::RegisterJob)?;
    println!("Job '{id}' registered.");

    Ok(())
}
