/// Why a command line is not an `ssh` invocation this worker can read.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum SshCommandError {
    #[error("the command does not start with ssh")]
    NotSsh,
    #[error("the -p option has no port")]
    MissingPort,
    #[error("{0:?} is not a port")]
    InvalidPort(String),
    #[error("the command names no host")]
    MissingHost,
}
