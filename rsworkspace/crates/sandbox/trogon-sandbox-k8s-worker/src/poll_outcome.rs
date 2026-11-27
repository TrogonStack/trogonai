use trogon_sandbox_providers::PodExecOutput;

/// Outcome of [`crate::K8sWorkerProvider::poll_command_execution`] for an
/// `execution_id` known to belong to a lease owned by this worker.
#[derive(Debug)]
pub enum PollOutcome {
    Running,
    Completed(PodExecOutput),
    Failed(String),
    NotFound,
}
