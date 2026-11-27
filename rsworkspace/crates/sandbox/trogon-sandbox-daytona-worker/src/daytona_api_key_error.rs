/// A Daytona API key was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum DaytonaApiKeyError {
    #[error("a Daytona API key is required")]
    Empty,
    #[error("the Daytona API key cannot be sent in an HTTP header")]
    NotAHeader,
}
