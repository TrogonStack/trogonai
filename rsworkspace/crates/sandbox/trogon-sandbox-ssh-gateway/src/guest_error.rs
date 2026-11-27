/// Why a guest could not serve a terminal.
#[derive(Debug, thiserror::Error)]
pub enum GuestError {
    #[error("ssh channel: {0}")]
    Ssh(#[from] russh::Error),
    /// The worker could not reach the sandbox, for a reason of its own.
    #[error("sandbox unavailable: {0}")]
    Unavailable(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the sandbox reported no exit status")]
    NoExitStatus,
}
