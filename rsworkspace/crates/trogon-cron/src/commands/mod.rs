use trogon_cron::{ConfigStore, JobEnabledState};

use crate::cli;

mod add;
mod get;
mod list;
mod remove;
mod set_state;

pub async fn handle_job<S>(
    store: S,
    action: cli::JobAction,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: ConfigStore,
{
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
