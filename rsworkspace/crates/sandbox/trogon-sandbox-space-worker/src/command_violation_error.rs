/// A command request this worker cannot serve, refused before SPACE is called.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum CommandViolationError {
    /// The caller needs launch cancellation, which SPACE cannot guarantee.
    #[error("this worker cannot guarantee that launch cancellation reaches the command")]
    LaunchCancellation,
    /// The command has no arguments.
    #[error("command must not be empty")]
    EmptyCommand,
    /// The timeout is outside the range SPACE accepts.
    #[error("timeout_seconds must be between 1 and 3600")]
    Timeout,
    /// The command is detached.
    #[error("detached commands are not supported by this worker")]
    Detached,
    /// The caller asks for streamed output.
    #[error("this worker cannot stream command output")]
    StreamOutput,
    /// The caller asks to run as another user.
    #[error("user overrides are not supported by this worker")]
    RunAsUser,
    /// The working directory holds a NUL byte.
    #[error("invalid working directory")]
    InvalidWorkingDirectory,
    /// A relative working directory has no lease directory to resolve against.
    #[error("a relative working directory needs the lease to set a working directory")]
    RelativeWorkingDirectory,
}
