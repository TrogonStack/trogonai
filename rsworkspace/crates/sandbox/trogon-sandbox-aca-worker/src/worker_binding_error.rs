/// Why a provider could not be bound to a worker.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum WorkerBindingError {
    /// The provider already serves another worker.
    #[error("provider is already bound to another worker")]
    AlreadyBound,
}
