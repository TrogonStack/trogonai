use std::sync::Arc;

use trogon_sandbox_contract::CommandOutput;

use super::command_task::CommandTask;

/// Where a detached execution stands.
pub(crate) enum ExecState {
    Running { task: Arc<CommandTask> },
    Done(CommandOutput),
}
