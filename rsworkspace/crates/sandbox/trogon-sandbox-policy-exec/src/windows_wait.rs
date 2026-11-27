//! Waits for the command to finish while forwarding an interrupt to it.

use std::future::Future;
use std::io;
use std::process::ExitStatus;

use tokio::process::Child;

/// How the command ended: it ran to its end, or an interrupt killed it,
/// which is reported as a clean exit.
#[derive(Debug)]
pub(crate) enum Outcome {
    Exited(ExitStatus),
    Interrupted,
}

impl Outcome {
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Exited(status) => status.code().unwrap_or(1),
            Self::Interrupted => 0,
        }
    }
}

pub(crate) async fn wait_forwarding(child: &mut Child, interrupt: impl Future<Output = ()>) -> io::Result<Outcome> {
    tokio::select! {
        status = child.wait() => status.map(Outcome::Exited),
        () = interrupt => {
            if let Err(error) = child.kill().await
                && child.try_wait()?.is_none()
            {
                return Err(error);
            }
            Ok(Outcome::Interrupted)
        }
    }
}

/// The console control events that stop a process on Windows: Ctrl+C and
/// Ctrl+Break, closing the console and logging off or shutting down.
#[cfg(windows)]
pub(crate) struct Interrupt {
    ctrl_c: tokio::signal::windows::CtrlC,
    ctrl_break: tokio::signal::windows::CtrlBreak,
    ctrl_close: tokio::signal::windows::CtrlClose,
    ctrl_logoff: tokio::signal::windows::CtrlLogoff,
    ctrl_shutdown: tokio::signal::windows::CtrlShutdown,
}

#[cfg(windows)]
impl Interrupt {
    pub(crate) fn register() -> io::Result<Self> {
        Ok(Self {
            ctrl_c: tokio::signal::windows::ctrl_c()?,
            ctrl_break: tokio::signal::windows::ctrl_break()?,
            ctrl_close: tokio::signal::windows::ctrl_close()?,
            ctrl_logoff: tokio::signal::windows::ctrl_logoff()?,
            ctrl_shutdown: tokio::signal::windows::ctrl_shutdown()?,
        })
    }

    pub(crate) async fn arrived(&mut self) {
        tokio::select! {
            _ = self.ctrl_c.recv() => {}
            _ = self.ctrl_break.recv() => {}
            _ = self.ctrl_close.recv() => {}
            _ = self.ctrl_logoff.recv() => {}
            _ = self.ctrl_shutdown.recv() => {}
        }
    }
}

#[cfg(test)]
mod tests;
