use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_sandbox_contract::{CommandDeadline, CommandExecution, RunCommandRequest};

use crate::LeaseSandbox;

/// The native exit a command was seen to reach.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeTerminal {
    pub(crate) exit: i32,
    pub(crate) deadline_kill_confirmed: bool,
}

/// Where the retained capture of a finished command is read back from.
#[derive(Debug, Clone)]
pub(crate) struct CompletionReceipt {
    pub(crate) source_id: String,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionState {
    pub(crate) terminal: Option<NativeTerminal>,
    pub(crate) completed: Option<CompletionReceipt>,
    pub(crate) cleanup_pending: bool,
}

/// One command this worker started, with what is needed to recover its output.
#[derive(Debug, Clone)]
pub(crate) struct Execution {
    pub(crate) lease_id: String,
    pub(crate) session_id: String,
    pub(crate) capture_id: String,
    pub(crate) max_output_bytes: usize,
    pub(crate) timeout_seconds: u32,
    pub(crate) deadline: CommandDeadline,
    pub(crate) sudo: bool,
    pub(crate) state: Arc<Mutex<ExecutionState>>,
}

impl Execution {
    pub(crate) fn new(sandbox: &LeaseSandbox, request: &RunCommandRequest, cap: usize) -> Self {
        let deadline = match &request.execution {
            CommandExecution::Attached => CommandDeadline::Timeout,
            CommandExecution::Detached { deadline, .. } => *deadline,
        };
        Self {
            lease_id: request.lease_id.as_str().to_owned(),
            session_id: sandbox.session_id.clone(),
            capture_id: uuid::Uuid::new_v4().simple().to_string().to_ascii_uppercase(),
            max_output_bytes: cap,
            timeout_seconds: request.timeout_seconds.max(1),
            deadline,
            sudo: request.run_as_user.is_some(),
            state: Arc::new(Mutex::new(ExecutionState::default())),
        }
    }
}

#[cfg(test)]
mod tests;
