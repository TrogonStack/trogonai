use trogon_cron::{JobEnabledState, NatsConfigStore};

use crate::cli;

mod add;
mod get;
mod job_id;
mod list;
mod remove;
mod set_state;

pub async fn handle_job(
    store: NatsConfigStore,
    action: cli::JobAction,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        cli::JobAction::List => list::run(&store, list::ListCommand)
            .await
            .map_err(Into::into),
        cli::JobAction::Get { id } => get::run(&store, get::GetCommand::try_from(id)?)
            .await
            .map_err(Into::into),
        cli::JobAction::Add => {
            let command = add::read_from_stdin()?;
            add::run(&store, command).await.map_err(Into::into)
        }
        cli::JobAction::Remove { id } => remove::run(&store, remove::RemoveCommand::try_from(id)?)
            .await
            .map_err(Into::into),
        cli::JobAction::Enable { id } => set_state::run(
            &store,
            set_state::SetStateCommand::new(id, JobEnabledState::Enabled)?,
        )
        .await
        .map_err(Into::into),
        cli::JobAction::Disable { id } => set_state::run(
            &store,
            set_state::SetStateCommand::new(id, JobEnabledState::Disabled)?,
        )
        .await
        .map_err(Into::into),
    }
}
