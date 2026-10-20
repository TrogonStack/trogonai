#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid authentication configuration")]
    Configuration,
    #[error("authentication required")]
    Unauthenticated,
    #[error("required scope is missing")]
    InsufficientScope,
    #[error("authentication service unavailable")]
    Unavailable,
}
