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
        cli::JobAction::List => list::run(&store).await.map_err(Into::into),
        cli::JobAction::Get { id } => get::run(&store, &id).await.map_err(Into::into),
        cli::JobAction::Add { file } => add::run(&store, &file).await.map_err(Into::into),
        cli::JobAction::Remove { id } => remove::run(&store, &id).await.map_err(Into::into),
        cli::JobAction::Enable { id } => set_state::run(&store, &id, JobEnabledState::Enabled)
            .await
            .map_err(Into::into),
        cli::JobAction::Disable { id } => set_state::run(&store, &id, JobEnabledState::Disabled)
            .await
            .map_err(Into::into),
    }
}
