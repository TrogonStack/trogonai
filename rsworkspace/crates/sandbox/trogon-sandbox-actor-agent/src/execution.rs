use tokio::sync::{Mutex, mpsc};
use trogon_sandbox_contract::{CommandOutput, UnixSignal};

use crate::stdin_handle::StdinHandle;

#[derive(Debug)]
pub(crate) enum ExecutionState {
    Running,
    Completed(CommandOutput),
}

/// One command started inside this actor.
#[derive(Debug)]
pub(crate) struct Execution {
    pub(crate) state: Mutex<ExecutionState>,
    /// Signals are delivered through the task that owns the child rather than to a raw pid, so a signal can never land
    /// on a pid the kernel has already recycled.
    pub(crate) kill: mpsc::Sender<UnixSignal>,
    pub(crate) stdin: Option<StdinHandle>,
}
