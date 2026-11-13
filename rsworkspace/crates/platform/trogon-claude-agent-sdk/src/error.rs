use std::{io, process::ExitStatus};

use crate::StderrTail;

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
    Session(#[from] crate::SessionError),
    #[error("could not start the Claude Code executable")]
    Spawn(#[source] io::Error),
    #[error("a piped subprocess stream was unavailable")]
    PipeUnavailable,
    #[error("Claude Code subprocess I/O failed")]
    Io(#[source] io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("protocol frame exceeds the configured byte limit ({limit})")]
    FrameTooLarge { limit: usize },
    #[error("Claude Code exited with status {status}")]
    Process { status: ExitStatus, stderr: StderrTail },
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
    #[error("blocking hooks require capability support from Claude Code")]
    BlockingHooksUnsupported,
    #[error("configured hooks were not registered by Claude Code")]
    HooksNotApplied,
    #[error("SDK configuration could not be encoded")]
    InvalidConfiguration,
    #[error("file checkpointing is not yet supported with a session store")]
    FileCheckpointingWithSessionStore,
    #[error("continue_conversation with a session store requires the store to support listing sessions")]
    ContinueRequiresListSessions,
    #[error("an MCP server replacement is already in progress")]
    McpUpdateInProgress,
    #[error("an MCP server name is configured as both external and embedded")]
    DuplicateMcpServer,
    #[error("the {operation} control request timed out")]
    ControlTimeout { operation: ControlOperation },
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
