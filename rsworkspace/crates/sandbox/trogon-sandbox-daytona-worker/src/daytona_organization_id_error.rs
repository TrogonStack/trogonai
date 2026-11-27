/// A Daytona organization id was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum DaytonaOrganizationIdError {
    #[error("a Daytona organization id must not be empty")]
    Empty,
    #[error("the Daytona organization id cannot be sent in an HTTP header")]
    NotAHeader,
}
