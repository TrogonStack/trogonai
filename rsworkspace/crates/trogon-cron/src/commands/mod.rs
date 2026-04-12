use async_nats::jetstream::{self, context, kv};
use trogon_cron::JobEnabledState;
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream, JetStreamPublishMessage};

use crate::cli;

mod add;
mod get;
mod list;
mod remove;
mod set_state;

pub async fn handle_job<J>(js: J, action: cli::JobAction) -> Result<(), Box<dyn std::error::Error>>
where
    J: JetStreamGetKeyValue<Store = kv::Store>
        + JetStreamGetStream<Stream = jetstream::stream::Stream>
        + JetStreamPublishMessage<
            PublishError = context::PublishError,
            AckFuture = context::PublishAckFuture,
        >,
{
    match action {
        cli::JobAction::List => list::run(&js, list::ListCommand).await.map_err(Into::into),
        cli::JobAction::Get { id } => get::run(&js, get::GetCommand::try_from(id)?)
            .await
            .map_err(Into::into),
        cli::JobAction::Add => {
            let command = add::read_from_stdin()?;
            add::run(&js, command).await.map_err(Into::into)
        }
        cli::JobAction::Remove { id } => remove::run(&js, remove::RemoveCommand::try_from(id)?)
            .await
            .map_err(Into::into),
        cli::JobAction::Enable { id } => set_state::run(
            &js,
            set_state::SetStateCommand::new(id, JobEnabledState::Enabled)?,
        )
        .await
        .map_err(Into::into),
        cli::JobAction::Disable { id } => set_state::run(
            &js,
            set_state::SetStateCommand::new(id, JobEnabledState::Disabled)?,
        )
        .await
        .map_err(Into::into),
    }
}
