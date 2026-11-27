use super::*;

/// State of a detached command execution started via
/// [`DockerWorkerProvider::run_command_for_lease_detached`], tracked so a
/// later `poll_command` request can retrieve it. Every request that names an
/// execution is scoped by `lease_id`: a lease that does not own it sees the
/// same answer as for an unknown execution.
#[derive(Clone)]
pub(super) enum ExecutionState {
    Running {
        lease_id: LeaseId,
        exec_id: Option<String>,
        container_id: String,
        stdin: Option<StdinHandle>,
        task: Option<Arc<CommandTask>>,
        stdin_task: Option<Arc<CommandTask>>,
        uncertainty: Option<String>,
    },
    Completed {
        lease_id: LeaseId,
        output: CommandOutput,
    },
}

impl ExecutionState {
    pub(super) fn lease_id(&self) -> &LeaseId {
        match self {
            Self::Running { lease_id, .. } | Self::Completed { lease_id, .. } => lease_id,
        }
    }
}
