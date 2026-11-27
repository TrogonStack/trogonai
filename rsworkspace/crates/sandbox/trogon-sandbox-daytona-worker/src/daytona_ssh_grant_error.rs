use crate::SshCommandError;

/// Why an SSH access reply from Daytona cannot be used.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum DaytonaSshGrantError {
    #[error("the ssh command is unusable: {0}")]
    Command(#[from] SshCommandError),
    #[error("the expiry is not an RFC 3339 timestamp")]
    Expiry,
}
