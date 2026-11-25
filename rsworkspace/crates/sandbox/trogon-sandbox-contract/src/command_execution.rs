use crate::{CommandDeadline, ProcessSpec, StdinMode};

/// How a command runs: to completion before the worker replies, or detached.
///
/// Only a detached command may keep its stdin open, run until its lease ends, or carry a [`ProcessSpec`], so those
/// options live in `Detached` and an attached command cannot name them.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub enum CommandExecution {
    /// The worker replies when the command completes.
    #[default]
    Attached,
    /// The worker replies as soon as it accepts the command, and the caller polls for the result.
    Detached {
        /// Whether the command's stdin stays open for stdin writes.
        stdin: StdinMode,
        /// How long the command may run.
        deadline: CommandDeadline,
        /// Names the process and attaches a readiness probe.
        process: Option<ProcessSpec>,
    },
}
