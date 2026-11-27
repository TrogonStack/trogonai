use trogon_sandbox_contract::{
    CommandEnv, OutputRetention, RedactValues, RunCommandRequest, SandboxUserName, StreamedOutput,
};

/// The knobs shared by every way of launching a command.
///
/// `Default` runs the command in the worker's default directory with no extra environment, the contract's default
/// timeout, no streamed output, buffered output retention, and nothing redacted.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommandOptions {
    /// Directory the command runs in. Absent, the worker keeps its own default.
    pub cwd: Option<String>,
    pub env: CommandEnv,
    /// How long the command may run, in seconds.
    pub timeout_seconds: u32,
    /// Runs the command as this user instead of the lease's configured user.
    pub run_as_user: Option<SandboxUserName>,
    /// Whether the worker publishes incremental output chunks while the command runs.
    pub stream_output: StreamedOutput,
    /// Whether the worker keeps the command's output for polling. Only a detached command can change it.
    pub output_retention: OutputRetention,
    /// Literals the worker replaces with `[REDACTED]` in everything it returns for this command. Only exact bytes
    /// are matched. A provider that cannot redact denies the launch instead of running it unfiltered.
    pub redact_values: RedactValues,
}

impl Default for CommandOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            env: CommandEnv::default(),
            timeout_seconds: RunCommandRequest::DEFAULT_TIMEOUT_SECONDS,
            run_as_user: None,
            stream_output: StreamedOutput::default(),
            output_retention: OutputRetention::Buffer,
            redact_values: RedactValues::default(),
        }
    }
}
