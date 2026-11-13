#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::{io, process::ExitStatus};

use crate::{JsonObject, StderrTail};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlOperation {
    Initialize,
    Interrupt,
    SetModel,
    SetPermissionMode,
    RewindFiles,
    McpStatus,
    ContextUsage,
    McpReconnect,
    McpToggle,
    McpReadResource,
    StopTask,
    BackgroundTasks,
    McpSetServers,
}

impl std::fmt::Display for ControlOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Initialize => "initialize",
            Self::Interrupt => "interrupt",
            Self::SetModel => "set_model",
            Self::SetPermissionMode => "set_permission_mode",
            Self::RewindFiles => "rewind_files",
            Self::McpStatus => "mcp_status",
            Self::ContextUsage => "get_context_usage",
            Self::McpReconnect => "mcp_reconnect",
            Self::McpToggle => "mcp_toggle",
            Self::McpReadResource => "mcp_read_resource",
            Self::StopTask => "stop_task",
            Self::BackgroundTasks => "background_tasks",
            Self::McpSetServers => "mcp_set_servers",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid JSON protocol frame")]
    Json(#[from] serde_json::Error),
    #[error("invalid permission update")]
    PermissionUpdate(#[from] crate::PermissionUpdateError),
    #[error("duplicate incoming control request ID")]
    DuplicateControlRequest,
    /// A message Python `parse_message` rejects, with its error text.
    #[error("{0}")]
    InvalidMessage(String),
}

#[derive(thiserror::Error)]
#[error("Claude Code rejected the control request")]
pub struct ControlRejectionError(pub(crate) String);

impl ControlRejectionError {
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ControlRejectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ControlRejectionError { .. }")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the CLI path cannot be started safely")]
    CliPath(#[from] crate::CliPathError),
    #[error(transparent)]
    CliNotFound(#[from] crate::CliNotFoundError),
    #[error(transparent)]
    Session(#[from] crate::SessionError),
    /// Python `CLIConnectionError("Failed to start Claude Code: ...")` for any spawn failure other than the two below.
    #[error("Failed to start Claude Code: {0}")]
    Spawn(#[source] io::Error),
    /// Python `CLIConnectionError("Working directory does not exist: ...")`, raised when the spawn finds no
    /// executable and the configured working directory is missing.
    #[error("Working directory does not exist: {}", .0.display())]
    WorkingDirectoryMissing(std::path::PathBuf),
    /// Python `CLINotFoundError("Claude Code not found at: ...")`, raised when the configured executable is missing.
    #[error("Claude Code not found at: {}", .0.display())]
    CliNotFoundAt(std::path::PathBuf),
    #[error("a piped subprocess stream was unavailable")]
    PipeUnavailable,
    #[error("Claude Code subprocess I/O failed")]
    Io(#[source] io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The CLI exited unsuccessfully. `Display` is Python's `ProcessError` text and `typescript_message` is the
    /// TypeScript text; `stderr` carries the real tail, a superset of both upstream messages.
    #[error("{}", process_message(.status))]
    Process { status: ExitStatus, stderr: StderrTail },
    /// Python `CLIConnectionError("ProcessTransport is not ready for writing")` and the TypeScript error of the
    /// same text: a write after stdin was closed.
    #[error("ProcessTransport is not ready for writing")]
    NotReadyForWriting,
    /// Python `CLIConnectionError("Cannot write to terminated process (exit code: N)")`. TypeScript's text carries no
    /// exit code, see `typescript_message`.
    #[error("Cannot write to terminated process (exit code: {})", .exit_code.map_or_else(|| "None".to_owned(), |code| code.to_string()))]
    WriteToTerminatedProcess { exit_code: Option<i32> },
    /// `Failed to write to process stdin: ...` in both upstream SDKs.
    #[error("Failed to write to process stdin: {0}")]
    WriteFailed(#[source] io::Error),
    /// Python `CLIJSONDecodeError`.
    #[error(transparent)]
    JsonDecode(#[from] JsonDecodeError),
    /// An option the upstream SDK rejects before it starts the CLI.
    #[error(transparent)]
    Option(#[from] crate::OptionError),
    /// Python `MessageParseError`.
    #[error(transparent)]
    MessageParse(#[from] MessageParseError),
    /// Python `ResultError` / TypeScript "Claude Code returned an error result": the CLI exited unsuccessfully
    /// after reporting an `is_error` result, so the result replaces the bare exit error.
    #[error(transparent)]
    Result(#[from] ResultError),
    /// TypeScript `AbortError`.
    #[error(transparent)]
    Aborted(#[from] AbortError),
    /// A failure reported by a custom [`crate::Transport`], carried unchanged. Python lets a transport raise any
    /// exception and the SDK propagates it as is.
    #[error(transparent)]
    Transport(Box<dyn std::error::Error + Send + Sync>),
    #[error("Claude Code closed stdout before completing the current operation")]
    UnexpectedEof,
    #[error("the message queue filled; consume messages or increase its capacity")]
    MessageQueueFull,
    #[error("too many outstanding control requests")]
    TooManyControls,
    #[error("too many background tasks in the SDK ledger")]
    TooManyTasks,
    #[error("input has been completed")]
    InputClosed,
    #[error("too many queued user inputs")]
    TooManyInputs,
    /// A hook set to block on failure, and the initialize response did not report `hook_on_failure_supported`.
    /// `Display` is Python's text, `typescript_message` TypeScript's.
    #[error(
        "The process answering initialize does not honor on_failure on hook matchers, so a hook that fails would not block. Update Claude Code if it is old, or remove on_failure=\"block\" from your hooks."
    )]
    BlockingHooksUnsupported,
    /// A hook set to block on failure, and the initialize response reported `hooks_applied: false`. `Display` is
    /// Python's text, `typescript_message` TypeScript's.
    #[error(
        "This session was already running and did not register your hooks, so a hook set to on_failure=\"block\" would never run. Start a new session, or remove on_failure=\"block\" from your hooks."
    )]
    HooksNotApplied,
    /// TypeScript: the initialize request carried `managedSettings` and the response did not report
    /// `managed_settings_applied: true`.
    #[error(
        "Claude Code did not confirm the managed settings sent with managedSettingsDelivery 'initialize', so the session was ended and no further prompt was sent. For a cloud session, use 'argv', because it does not read this delivery. Otherwise check that no wrapper changes the launch flags or the initialize request."
    )]
    ManagedSettingsNotApplied,
    /// TypeScript `supportedDialogKinds` without `onUserDialog`.
    #[error(
        "supportedDialogKinds requires an onUserDialog callback -- declaring dialog kinds without a handler would park dialogs nothing can answer. Provide onUserDialog, or omit supportedDialogKinds."
    )]
    DialogKindsWithoutHandler,
    #[error("SDK configuration could not be encoded")]
    InvalidConfiguration,
    #[error("file checkpointing is not yet supported with a session store")]
    FileCheckpointingWithSessionStore,
    #[error("continue_conversation with a session store requires the store to support listing sessions")]
    ContinueRequiresListSessions,
    /// Python's text for a control request that went unanswered for the configured wait.
    #[error("Control request timeout: {operation}")]
    ControlTimeout { operation: ControlOperation },
    /// TypeScript's text when initialize goes unanswered while a hook is set to block on failure.
    #[error(
        "Claude Code did not answer initialize, so a hook set to onFailure: \"block\" could not be confirmed and the prompt was not sent. Check the connection and start a new session."
    )]
    InitializeNotConfirmed,
    /// TypeScript's text when initialize goes unanswered after carrying managed settings and no hook blocks.
    #[error(
        "Claude Code did not answer initialize, so the managed settings could not be confirmed and the prompt was not sent. Start a new session."
    )]
    ManagedSettingsNotConfirmed,
    /// Python's `int()` error for a `CLAUDE_CODE_STREAM_CLOSE_TIMEOUT` that is not an integer.
    #[error("invalid literal for int() with base 10: {literal}")]
    InvalidStreamCloseTimeout { literal: String },
    #[error("the {operation} control request was rejected")]
    ControlRejected {
        operation: ControlOperation,
        #[source]
        rejection: ControlRejectionError,
    },
    #[error("subprocess write timed out")]
    WriteTimeout,
    #[error("the command queue did not accept a request before its deadline")]
    CommandQueueTimeout,
    #[error("Claude Code could not be reaped before the shutdown deadline")]
    ShutdownTimeout,
    #[error("stderr did not close before the shutdown deadline")]
    StderrDrainTimeout,
    #[error("cleanup failed after an SDK error")]
    Cleanup {
        primary: Box<Error>,
        #[source]
        cleanup: Box<Error>,
    },
    #[error("the SDK driver has stopped; receive remaining messages for its terminal error")]
    DriverStopped,
    #[error("the client is not initialized")]
    NotInitialized,
    #[error("control request counter exhausted")]
    RequestIdExhausted,
    #[error("SDK driver task failed")]
    DriverTask(#[source] tokio::task::JoinError),
    #[error("permission callback task failed")]
    CallbackTask(#[source] tokio::task::JoinError),
    #[doc(hidden)]
    #[error("the subprocess owner requested shutdown")]
    ShutdownRequested,
}

fn process_message(status: &ExitStatus) -> String {
    let code = exit_code(status).unwrap_or(-1);
    format!("Command failed with exit code {code} (exit code: {code})\nError output: Check stderr output for details")
}

/// Python's `Process.returncode`: the exit code, or the negated signal number on Unix.
pub(crate) fn exit_code(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        status.code().or_else(|| status.signal().map(|signal| -signal))
    }
    #[cfg(not(unix))]
    {
        status.code()
    }
}

impl Error {
    /// The exit code Python's `ProcessError.exit_code` carries (the negated signal number on Unix).
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Process { status, .. } => exit_code(status),
            Self::Result(result) => result.exit_code(),
            _ => None,
        }
    }

    /// TypeScript's text for an unsuccessful exit: `Claude Code process exited with code N. stderr: <tail>` or
    /// `Claude Code process terminated by signal SIG<NAME>`, with the stderr tail appended when it is not blank.
    pub fn typescript_message(&self) -> Option<String> {
        match self {
            Self::WriteToTerminatedProcess { .. } => return Some("Cannot write to terminated process".to_owned()),
            Self::BlockingHooksUnsupported => {
                return Some(
                    "The process answering initialize does not honor onFailure on hook matchers, so a hook that fails would not block. Update Claude Code if it is old, or remove onFailure: \"block\" from your hooks."
                        .to_owned(),
                );
            }
            Self::HooksNotApplied => {
                return Some(
                    "This session was already running and did not register your hooks, so a hook set to onFailure: \"block\" would never run. Start a new session, or remove onFailure: \"block\" from your hooks."
                        .to_owned(),
                );
            }
            _ => {}
        }
        let Self::Process { status, stderr } = self else {
            return None;
        };
        let tail = String::from_utf8_lossy(stderr.as_bytes());
        let tail = tail.trim();
        let tail = if tail.is_empty() {
            String::new()
        } else {
            format!(". stderr: {tail}")
        };
        if let Some(code) = status.code() {
            return Some(format!("Claude Code process exited with code {code}{tail}"));
        }
        #[cfg(unix)]
        {
            if let Some(signal) = status.signal() {
                return Some(format!(
                    "Claude Code process terminated by signal {}{tail}",
                    signal_name(signal)
                ));
            }
        }
        None
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> &'static str {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => "unknown",
    }
}

/// Python `CLIJSONDecodeError`: a CLI stdout line that is not valid JSON, or a message that outgrew
/// `max_buffer_size`.
#[derive(Debug, thiserror::Error)]
#[error("Failed to decode JSON: {}...", self.line.chars().take(100).collect::<String>())]
pub struct JsonDecodeError {
    line: String,
    #[source]
    original_error: JsonDecodeSourceError,
}

/// The exception Python carries as `CLIJSONDecodeError.original_error`.
#[derive(Debug, thiserror::Error)]
pub enum JsonDecodeSourceError {
    /// The JSON parser's error for a corrupt line.
    #[error(transparent)]
    Json(serde_json::Error),
    /// Python's `ValueError` raised when one message exceeds the buffer limit.
    #[error("Buffer size {length} exceeds limit {limit}")]
    BufferExceeded { length: usize, limit: usize },
}

impl JsonDecodeError {
    pub(crate) fn new(line: &[u8], original_error: serde_json::Error) -> Self {
        Self {
            line: String::from_utf8_lossy(line).into_owned(),
            original_error: JsonDecodeSourceError::Json(original_error),
        }
    }

    /// Python's guard for a message longer than `max_buffer_size`: the text is the `line`, the `ValueError` the
    /// original error.
    pub(crate) fn buffer_exceeded(limit: usize, length: usize) -> Self {
        Self {
            line: format!("JSON message exceeded maximum buffer size of {limit} bytes"),
            original_error: JsonDecodeSourceError::BufferExceeded { length, limit },
        }
    }

    /// The offending line, complete.
    pub fn line(&self) -> &str {
        &self.line
    }

    pub fn original_error(&self) -> &JsonDecodeSourceError {
        &self.original_error
    }
}

/// Python `MessageParseError`: a JSON frame that does not decode as a message, with the raw frame.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct MessageParseError {
    message: String,
    data: Option<JsonObject>,
    #[source]
    source: ProtocolError,
}

impl MessageParseError {
    pub(crate) fn new(source: ProtocolError, data: Option<JsonObject>) -> Self {
        Self {
            message: source.to_string(),
            data,
            source,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// The decoded JSON object the message was parsed from, when the line was an object.
    pub fn data(&self) -> Option<&JsonObject> {
        self.data.as_ref()
    }
}

/// TypeScript `AbortError`: the operation was cancelled by the caller.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct AbortError {
    message: &'static str,
}

impl AbortError {
    /// The process was stopped by the caller (`Claude Code process aborted by user`).
    pub(crate) const PROCESS: Self = Self {
        message: "Claude Code process aborted by user",
    };
    /// A write or other operation attempted after the abort (`Operation aborted`).
    pub(crate) const OPERATION: Self = Self {
        message: "Operation aborted",
    };

    pub fn message(&self) -> &'static str {
        self.message
    }
}

/// Python `ResultError`: the payload of the error `result` the CLI reported before exiting unsuccessfully.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ResultError {
    message: String,
    data: JsonObject,
    exit_code: Option<i32>,
}

impl ResultError {
    pub(crate) fn new(message: String, data: JsonObject, exit_code: Option<i32>) -> Self {
        Self {
            message,
            data,
            exit_code,
        }
    }

    /// `Claude Code returned an error result: <text>`.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The raw `result` frame as the CLI emitted it.
    pub fn data(&self) -> &JsonObject {
        &self.data
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn subtype(&self) -> Option<&str> {
        self.data.get("subtype").and_then(serde_json::Value::as_str)
    }

    /// The `errors` strings, trimmed and without blanks; a bare string is accepted like Python does.
    pub fn errors(&self) -> Vec<String> {
        normalized_errors(self.data.get("errors"))
    }

    pub fn result(&self) -> Option<&str> {
        self.data.get("result").and_then(serde_json::Value::as_str)
    }

    pub fn api_error_status(&self) -> Option<i64> {
        self.data.get("api_error_status").and_then(serde_json::Value::as_i64)
    }

    pub fn terminal_reason(&self) -> Option<&str> {
        self.data.get("terminal_reason").and_then(serde_json::Value::as_str)
    }

    pub fn session_id(&self) -> Option<&str> {
        self.data.get("session_id").and_then(serde_json::Value::as_str)
    }
}

fn normalized_errors(raw: Option<&serde_json::Value>) -> Vec<String> {
    let strings: Vec<&str> = match raw {
        Some(serde_json::Value::String(text)) => vec![text.as_str()],
        Some(serde_json::Value::Array(items)) => items.iter().filter_map(serde_json::Value::as_str).collect(),
        _ => Vec::new(),
    };
    strings
        .into_iter()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Python `_error_result_text`: errors, then result, then a non-success subtype, then the HTTP status.
pub(crate) fn python_error_result_text(data: &JsonObject) -> String {
    let errors = normalized_errors(data.get("errors"));
    if !errors.is_empty() {
        return errors.join("; ");
    }
    if let Some(result) = data.get("result").and_then(serde_json::Value::as_str)
        && !result.trim().is_empty()
    {
        return result.trim().to_owned();
    }
    if let Some(subtype) = data.get("subtype").and_then(serde_json::Value::as_str)
        && !subtype.is_empty()
        && subtype != "success"
    {
        return subtype.to_owned();
    }
    if let Some(status) = data.get("api_error_status")
        && !status.is_null()
    {
        let status = status
            .as_i64()
            .map_or_else(|| status.to_string(), |number| number.to_string());
        return format!("API error (HTTP {status})");
    }
    "unknown error".to_owned()
}

/// TypeScript's text: the `result` for the `success` subtype, otherwise the joined `errors`; blank means none.
pub(crate) fn typescript_error_result_text(data: &JsonObject) -> Option<String> {
    let text = if data.get("subtype").and_then(serde_json::Value::as_str) == Some("success") {
        data.get("result")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    } else {
        normalized_errors(data.get("errors")).join("; ")
    };
    (!text.is_empty()).then_some(text)
}

/// The `result` frame as a JSON object, for `ResultError::data`.
pub(crate) fn error_result_data(result: &crate::ResultMessage) -> JsonObject {
    let mut data = result.additional_fields.clone();
    data.insert("type".into(), "result".into());
    data.insert("subtype".into(), result.subtype.clone().into());
    data.insert("is_error".into(), true.into());
    data.insert("session_id".into(), result.session_id.as_str().into());
    if let Some(text) = &result.result {
        data.insert("result".into(), text.clone().into());
    }
    if let Some(errors) = &result.errors {
        data.insert("errors".into(), errors.clone().into());
    }
    if let Some(status) = result.api_error_status {
        data.insert("api_error_status".into(), status.into());
    }
    if let Some(reason) = &result.terminal_reason {
        data.insert("terminal_reason".into(), reason.clone().into());
    }
    data
}
