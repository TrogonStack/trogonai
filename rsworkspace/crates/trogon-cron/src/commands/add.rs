use std::{fmt, io::Read};

use async_nats::jetstream::{self, context, kv};
use trogon_cron::{
    CronError, JobDecisionError, JobSpec, JobStreamState, JobWriteCondition, PutJobCommand,
    SNAPSHOT_STORE_CONFIG, VersionedJobSpec, append_events, initial_state, open_snapshot_bucket,
};
use trogon_eventsourcing::{Decision, decide, load_snapshot};
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

#[derive(Debug)]
pub struct AddCommand {
    command: PutJobCommand,
}

#[derive(Debug)]
pub enum CommandError {
    ReadStdin(std::io::Error),
    DeserializeJobSpec(serde_json::Error),
    InvalidJobSpec(CronError),
    LoadJob(CronError),
    RegisterJob(CronError),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadStdin(source) => write!(f, "failed to read stdin: {source}"),
            Self::DeserializeJobSpec(source) => write!(f, "invalid job spec payload: {source}"),
            Self::InvalidJobSpec(source) => write!(f, "invalid job spec: {source}"),
            Self::LoadJob(source) => write!(f, "failed to load current job state: {source}"),
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
            Self::LoadJob(source) => Some(source),
            Self::RegisterJob(source) => Some(source),
        }
    }
}

impl AddCommand {
    pub fn new(spec: JobSpec) -> Result<Self, CommandError> {
        Ok(Self {
            command: PutJobCommand::new(spec, JobWriteCondition::MustNotExist)
                .map_err(CommandError::InvalidJobSpec)?,
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
    let id = command.command.id().to_string();
    let bucket = open_snapshot_bucket(js)
        .await
        .map_err(CommandError::LoadJob)?;
    let current_snapshot = load_snapshot::<VersionedJobSpec>(&bucket, SNAPSHOT_STORE_CONFIG, &id)
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
    let events = match decide(&current_state, &command.command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CommandError::RegisterJob(CronError::event_source(
                "failed to decide job registration from current stream state",
                std::io::Error::other("unsupported decision variant"),
            )));
        }
        Err(JobDecisionError::CannotRegisterExistingJob { .. }) => {
            return Err(CommandError::RegisterJob(
                CronError::OptimisticConcurrencyConflict {
                    id: id.clone(),
                    expected: JobWriteCondition::MustNotExist,
                    current_version: current_snapshot.as_ref().map(|job| job.version),
                },
            ));
        }
        Err(error) => {
            return Err(CommandError::RegisterJob(CronError::event_source(
                "failed to decide job registration from current stream state",
                error,
            )));
        }
    };

    append_events(
        js,
        &id,
        JobWriteCondition::MustNotExist,
        events.map(trogon_cron::JobEventData::new),
    )
    .await
    .map_err(CommandError::RegisterJob)?;
    println!("Job '{id}' registered.");

    Ok(())
}
