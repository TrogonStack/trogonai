/// Why the command launch authority of an [`E2bExec`](super::E2bExec) cannot be configured.
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("worker command launch fence is already configured")]
    WorkerFenceAlreadyConfigured,
    #[error("command launch custody is already configured")]
    CustodyAlreadyConfigured,
}
