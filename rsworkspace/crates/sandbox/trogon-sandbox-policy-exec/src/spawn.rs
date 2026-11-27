//! A command that runs while its caller does something else.

use std::process::ExitStatus;

use nix::sys::signal::Signal;
use nix::unistd::Pid;
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_egress_proxy::CommandId;

use crate::error::RuntimeError;

/// How a spawned command ended.
#[derive(Debug, Clone)]
pub struct CommandExit {
    pub command: CommandId,
    pub status: ExitStatus,
    pub timed_out: bool,
}

/// A command started by [`crate::PolicyExec::spawn`]. Its pipes are the
/// ones the spec asked for; dropping the handle does not stop the command.
pub struct RunningCommand {
    pub(crate) id: CommandId,
    pub(crate) pid: Option<u32>,
    pub(crate) stdin: Option<ChildStdin>,
    pub(crate) stdout: Option<ChildStdout>,
    pub(crate) stderr: Option<ChildStderr>,
    pub(crate) cancel: CancellationToken,
    pub(crate) exit: oneshot::Receiver<Result<CommandExit, RuntimeError>>,
}

impl RunningCommand {
    pub fn id(&self) -> &CommandId {
        &self.id
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.stderr.take()
    }

    /// Kills the command. [`Self::wait`] then reports how it ended.
    pub fn kill(&self) {
        self.cancel.cancel();
    }

    /// Whether the signal was delivered.
    pub fn signal(&self, signal: Signal) -> bool {
        self.pid
            .and_then(|pid| i32::try_from(pid).ok())
            .is_some_and(|pid| nix::sys::signal::kill(Pid::from_raw(pid), signal).is_ok())
    }

    /// Waits for the command to end and its proxy and scratch files to be
    /// released.
    pub async fn wait(self) -> Result<CommandExit, RuntimeError> {
        self.exit
            .await
            .map_err(|_| RuntimeError::Launch(anyhow::anyhow!("the command supervisor stopped early")))?
    }
}
