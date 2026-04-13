use std::{fmt, io::Read};

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    CronError, JobEvent, JobEventData, JobSpec, JobWriteCondition, ResolvedJobSpec, append_events,
};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct AddCommand {
    spec: ResolvedJobSpec,
}

#[derive(Debug)]
pub enum CommandError {
    ReadStdin(std::io::Error),
    DeserializeJobSpec(serde_json::Error),
    InvalidJobSpec(CronError),
    RegisterJob(CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadStdin(source) => write!(f, "failed to read stdin: {source}"),
            Self::DeserializeJobSpec(source) => write!(f, "invalid job spec payload: {source}"),
            Self::InvalidJobSpec(source) => write!(f, "invalid job spec: {source}"),
            Self::RegisterJob(source) => write!(f, "failed to register job: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadStdin(source) => Some(source),
            Self::DeserializeJobSpec(source) => Some(source),
            Self::InvalidJobSpec(source) => Some(source),
            Self::RegisterJob(source) => Some(source),
        }
    }
}

impl AddCommand {
    pub fn new(spec: JobSpec) -> Result<Self, CommandError> {
        Ok(Self {
            spec: ResolvedJobSpec::try_from(&spec).map_err(CommandError::InvalidJobSpec)?,
        })
    }
}

pub fn read_from_stdin() -> Result<AddCommand, CommandError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(CommandError::ReadStdin)?;

    let spec = serde_json::from_str(&buf).map_err(CommandError::DeserializeJobSpec)?;
    AddCommand::new(spec)
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
    let id = command.spec.id().to_string();
    append_events(
        js,
        &id,
        JobWriteCondition::MustNotExist,
        JobEventData::new(JobEvent::job_registered(command.spec.spec().clone())),
    )
    .await
    .map_err(CommandError::RegisterJob)?;
    println!("Job '{id}' registered.");

    Ok(())
}
