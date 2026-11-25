#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegistrationError {
    #[error("registration contains an invalid identity or scope")]
    Binding,
    #[error("registration resource bounds are invalid")]
    Bounds,
    #[error("registration freshness is invalid")]
    Freshness,
    #[error("registration recovery fence or configured scope differs")]
    Scope,
    #[error("registration revision is stale or equivocal")]
    Revision,
    #[error("identity state is unavailable")]
    Unavailable,
}
