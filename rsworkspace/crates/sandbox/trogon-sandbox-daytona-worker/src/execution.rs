use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use trogon_sandbox_contract::CommandOutput;

use crate::ordinary_execution::OrdinaryExecution;
use crate::side_files::SideFiles;
use crate::stdin_state::StdinState;

/// One detached command this worker started.
#[derive(Debug, Clone)]
pub(crate) struct Execution {
    pub(crate) lease_id: String,
    pub(crate) sandbox_id: String,
    pub(crate) session_id: String,
    pub(crate) command_id: String,
    pub(crate) files: SideFiles,
    pub(crate) max_output_bytes: usize,
    /// Whether the command runs under a deadline that leaves a receipt in `files.status`.
    pub(crate) has_deadline: bool,
    /// Set when the command was started with interactive stdin.
    pub(crate) stdin: Option<Arc<StdinState>>,
    /// Set when the command runs under the installed ordinary command runtime, which then owns its signals, stdin
    /// and completion.
    pub(crate) ordinary: Option<Arc<OrdinaryExecution>>,
    /// The result of the first poll that saw the command end. Its side files and session are removed then, so later
    /// polls answer from here.
    pub(crate) completed: Option<CommandOutput>,
    /// When `completed` was recorded, which bounds how long it is kept.
    pub(crate) completed_at: Option<Instant>,
}

/// Drops finished results older than `ttl`, then the oldest ones beyond `max`. Running commands are never touched.
pub(crate) fn evict_completed(executions: &mut HashMap<String, Execution>, now: Instant, ttl: Duration, max: usize) {
    executions.retain(|_, execution| {
        execution
            .completed_at
            .is_none_or(|at| now.saturating_duration_since(at) < ttl)
    });
    let mut done: Vec<(Instant, String)> = executions
        .iter()
        .filter_map(|(id, execution)| execution.completed_at.map(|at| (at, id.clone())))
        .collect();
    if done.len() > max {
        done.sort();
        for (_, id) in &done[..done.len() - max] {
            executions.remove(id);
        }
    }
}

#[cfg(test)]
mod tests;
