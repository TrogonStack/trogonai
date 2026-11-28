use trogon_sandbox_contract::LeaseError;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ExitState {
    Running,
    Exited(Option<i32>),
    /// The sandbox refused the poll, so the process state is unknown; the reason is kept.
    Denied(Box<LeaseError>),
}
