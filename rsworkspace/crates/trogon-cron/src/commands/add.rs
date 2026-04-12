use std::{fmt, io::Read};

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{JobSpec, PutJobCommand, put_job};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct AddCommand {
    pub spec: JobSpec,
}

#[derive(Debug)]
pub enum CommandError {
    ReadStdin(std::io::Error),
    InvalidJobSpec(serde_json::Error),
    PutJob(trogon_cron::CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadStdin(source) => write!(f, "failed to read stdin: {source}"),
            Self::InvalidJobSpec(source) => write!(f, "invalid job spec: {source}"),
            Self::PutJob(source) => write!(f, "failed to register job: {source}"),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadStdin(source) => Some(source),
            Self::InvalidJobSpec(source) => Some(source),
            Self::PutJob(source) => Some(source),
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
    put_job(
        js,
        PutJobCommand {
            spec: command.spec,
            write_condition: trogon_cron::JobWriteCondition::MustNotExist,
        },
    )
        .await
        .map_err(CommandError::PutJob)?;
    println!("Job '{id}' registered.");

    Ok(())
}
