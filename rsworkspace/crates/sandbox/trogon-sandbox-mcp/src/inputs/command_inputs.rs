use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One literal to scrub from command output. `Debug` hides it so a logged tool input never carries the value.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(crate) struct RedactValueInput(pub String);

impl std::fmt::Debug for RedactValueInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedactValueInput(***)")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RunCommandInput {
    pub lease_id: String,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub timeout_seconds: u32,
    /// Literal values to replace with `[REDACTED]` in stdout, stderr and error text before they are returned or
    /// streamed. Exact bytes only: an encoded or transformed copy of a value is not caught, and at most 64 values
    /// are accepted. Requires a provider whose capability profile has `supports_output_redaction`.
    #[serde(default)]
    pub redact_values: Vec<RedactValueInput>,
    /// When `true`, returns immediately with an `execution_id` to poll via `sandbox_poll_command` instead of
    /// blocking until the command finishes.
    #[serde(default)]
    pub detached: Option<bool>,
    /// Runs the command as this sandbox user instead of the lease's default user.
    #[serde(default)]
    pub user: Option<String>,
    /// When `true`, the worker publishes incremental output chunks to the evidence stream as the command runs.
    /// Defaults to `false` when omitted.
    #[serde(default)]
    pub stream_output: Option<bool>,
    /// When `true` and the client sent a `progressToken`, the call reports the command's output as MCP progress
    /// notifications (one per chunk, stderr marked `[stderr]`) while it runs. The result is the same as without
    /// it. Needs the broker to keep command logs; otherwise no progress is sent. Requires `detached=false`.
    #[serde(default)]
    pub stream_progress: Option<bool>,
    /// `timeout` (default) bounds the command by `timeout_seconds`; `lease` lets a detached command live until its
    /// lease ends.
    #[serde(default)]
    pub deadline: Option<CommandDeadlineInput>,
    /// `closed` (default) gives the command no stdin; `interactive` keeps it open for `sandbox_write_stdin` and
    /// requires `detached=true` on a provider whose capability profile has `supports_interactive_stdin`.
    #[serde(default)]
    pub stdin: Option<StdinModeInput>,
    /// `buffer` (default) keeps stdout and stderr for `sandbox_poll_command`; `stream_only` keeps no stdout and
    /// only a stderr tail, for long-lived processes whose output is read through `stream_output`.
    #[serde(default)]
    pub output_retention: Option<OutputRetentionInput>,
    /// Gives a detached command a caller-chosen name (unique per lease) and an optional readiness probe. Starting
    /// a name that is already running returns the existing `execution_id`. Requires `detached=true` and a provider
    /// whose capability profile has `supports_named_processes` (and `supports_process_readiness` for a probe).
    #[serde(default)]
    pub process: Option<ProcessInput>,
    /// Makes a retried call safe: the broker runs the command once per key and lease, and a later call with the
    /// same key replays the first launch's reply instead of running it again. 1 to 128 characters from
    /// `A-Z a-z 0-9 _ -`. Without it, every call runs the command.
    #[serde(default)]
    pub launch_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ProcessInput {
    /// 1 to 64 characters from `A-Z a-z 0-9 . _ -`.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub readiness: Option<ReadinessInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum ReadinessInput {
    /// Ready once something listens on this TCP port inside the sandbox.
    TcpPort { port: u16 },
    /// Ready once stdout or stderr has contained this text.
    OutputContains { needle: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandDeadlineInput {
    Timeout,
    Lease,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StdinModeInput {
    Closed,
    Interactive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputRetentionInput {
    Buffer,
    StreamOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ListProcessesInput {
    pub lease_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WaitReadyInput {
    pub lease_id: String,
    pub execution_id: String,
    /// How long to wait for the probe, in seconds. Defaults to 30 and is capped at 300.
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct KillProcessInput {
    pub lease_id: String,
    pub name: String,
    /// One of `"term"`, `"kill"`, `"int"`, `"hup"`, `"quit"`, `"usr1"`, `"usr2"`. Defaults to `"term"` when omitted.
    #[serde(default)]
    pub signal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WriteStdinInput {
    pub lease_id: String,
    pub execution_id: String,
    /// Position of this write within the execution, starting at 0 with no gaps. Retrying a failed call with the
    /// same `seq` is safe.
    pub seq: u64,
    /// At most 64 KiB once decoded.
    #[serde(default)]
    pub data_base64: String,
    /// When `true`, closes the command's stdin after `data_base64` is written.
    #[serde(default)]
    pub eof: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub(crate) enum RunCommandOutput {
    Completed {
        exit_code: i32,
        stdout_base64: String,
        stderr_base64: String,
    },
    Started {
        execution_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct PollCommandInput {
    pub lease_id: String,
    pub execution_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum PollCommandOutput {
    Running,
    Completed {
        exit_code: i32,
        stdout_base64: String,
        stderr_base64: String,
    },
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct KillCommandInput {
    pub lease_id: String,
    pub execution_id: String,
    /// One of `"term"`, `"kill"`, `"int"`, `"hup"`, `"quit"`, `"usr1"`, `"usr2"`. Defaults to `"term"` when omitted.
    #[serde(default)]
    pub signal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct StopCommandInput {
    pub lease_id: String,
    pub execution_id: String,
    /// Seconds to wait after SIGTERM before SIGKILL. Defaults to 10, capped at 300.
    #[serde(default)]
    pub grace_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct RestartCommandInput {
    pub lease_id: String,
    pub execution_id: String,
    /// Seconds to wait after SIGTERM before SIGKILL. Defaults to 10, capped at 300.
    #[serde(default)]
    pub grace_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ReadLogsInput {
    pub lease_id: String,
    pub execution_id: String,
    /// Cursor from a previous page's `next_seq`; 0 reads from the oldest retained chunk.
    #[serde(default)]
    pub after_seq: u64,
    /// Page size in bytes. Defaults to 64 KiB, capped at 512 KiB.
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct LogChunkOutput {
    pub log_seq: u64,
    pub stream: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub(crate) enum ReadLogsOutput {
    Logs {
        chunks: Vec<LogChunkOutput>,
        next_seq: u64,
        has_more: bool,
        truncated: bool,
    },
    Unsupported,
}
