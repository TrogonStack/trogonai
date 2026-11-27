use trogon_sandbox_contract::LeaseId;

use crate::StdinHandle;
use crate::command_execution_state::CommandExecutionState;
use crate::execution_task::ExecutionTask;

pub(crate) struct TrackedExecution {
    pub(crate) lease_id: LeaseId,
    pub(crate) state: CommandExecutionState,
    pub(crate) stdin: Option<StdinHandle>,
    pub(crate) tasks: Vec<ExecutionTask>,
}
