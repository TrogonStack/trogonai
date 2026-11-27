use std::convert::Infallible;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};

use nix::fcntl::{FcntlArg, FdFlag, OFlag, fcntl};
use nix::pty::{OpenptyResult, Winsize, openpty};
use tokio::io::unix::AsyncFd;
use tokio::process::{Child, Command};
use trogon_sandbox_contract::TerminalError;

use crate::syscall_result::syscall_result;

fn spawn_failed(reason: String) -> TerminalError {
    TerminalError::SpawnFailed { reason }
}

pub(crate) fn winsize(cols: u16, rows: u16) -> Winsize {
    Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

pub(crate) fn set_size(master: &OwnedFd, cols: u16, rows: u16) -> std::io::Result<()> {
    let size = winsize(cols, rows);
    // SAFETY: `TIOCSWINSZ` reads one `winsize` from a valid pointer.
    syscall_result(unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) })
}

/// Starts a new session and makes `terminal` its controlling terminal.
///
/// It runs between fork and exec of the terminal's command, with the pseudo-terminal's slave as descriptor 0.
pub(crate) fn become_session_leader(terminal: RawFd) -> std::io::Result<()> {
    let started = nix::unistd::setsid().map_err(std::io::Error::from);
    started.and_then(|_| claim_controlling_terminal(terminal))
}

/// Makes `terminal` the controlling terminal of the calling session.
fn claim_controlling_terminal(terminal: RawFd) -> std::io::Result<()> {
    // SAFETY: `TIOCSCTTY` takes an integer argument and no pointer.
    syscall_result(unsafe { libc::ioctl(terminal, libc::TIOCSCTTY as _, 0) })
}

/// Wires every standard stream of `command` to the slave side.
///
/// `openpty` leaves both ends inheritable, and a command spawned concurrently would keep the slave open so this terminal
/// would never see its own command end; the duplicates handed to the command are the only ones that may survive.
fn attach(command: &mut Command, pty: &OpenptyResult) -> std::io::Result<()> {
    fcntl(&pty.master, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
    fcntl(&pty.slave, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
    fcntl(&pty.master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
    command.stdin(pty.slave.try_clone()?);
    command.stdout(pty.slave.try_clone()?);
    command.stderr(pty.slave.try_clone()?);
    // SAFETY: only async-signal-safe calls run between fork and exec.
    unsafe { command.pre_exec(|| become_session_leader(0)) };
    Ok(())
}

/// Registers the master side with the runtime.
///
/// The registration is handed an owned descriptor that nothing else closes, which is the condition the deprecated
/// constructor's warning is about, and the replacement it points to is not released yet.
#[allow(deprecated)]
fn watch(master: OwnedFd) -> std::io::Result<AsyncFd<OwnedFd>> {
    AsyncFd::new(master)
}

/// Starts `command` on a new pseudo-terminal of the given size and returns it with the master side.
pub(crate) fn spawn_on_pty(
    mut command: Command,
    program: &str,
    cols: u16,
    rows: u16,
) -> Result<(Child, AsyncFd<OwnedFd>), TerminalError> {
    let pty = openpty(Some(&winsize(cols, rows)), None);
    let pty = pty.map_err(|error| spawn_failed(format!("allocating a pseudo-terminal: {error}")))?;
    let prepared = attach(&mut command, &pty).and_then(|()| watch(pty.master));
    let master = prepared.map_err(|error| spawn_failed(format!("preparing the pseudo-terminal: {error}")))?;
    let child = command.spawn();
    let child = child.map_err(|error| spawn_failed(format!("starting `{program}`: {error}")))?;
    // The parent's copies of the slave must close, or the master never sees the end.
    drop(command);
    drop(pty.slave);
    Ok((child, master))
}

pub(crate) async fn read_some(master: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        let mut ready = master.readable().await?;
        if let Ok(result) = ready.try_io(|fd| nix::unistd::read(fd.get_ref(), &mut *buf).map_err(std::io::Error::from))
        {
            return result;
        }
    }
}

pub(crate) async fn write_all(master: &AsyncFd<OwnedFd>, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        let mut ready = master.writable().await?;
        let attempt = ready.try_io(|fd| nix::unistd::write(fd.get_ref(), bytes).map_err(std::io::Error::from));
        // A spurious wakeup writes nothing and the loop waits again.
        let written = attempt.unwrap_or(Ok(0))?;
        bytes = &bytes[written..];
    }
    Ok(())
}

/// Reads and drops whatever the terminal produces, and never completes.
///
/// Some kernels make a process wait in `exit` until the output it left on its terminal has been drained, so a master
/// nobody reads would hold the exit up even past a kill.
pub(crate) async fn discard_forever(master: &AsyncFd<OwnedFd>) -> Infallible {
    let mut sink = [0u8; 4096];
    loop {
        let more = read_some(master, &mut sink).await.is_ok_and(|count| count > 0);
        wait_for(more).await;
    }
}

/// Returns at once when `ready`, and otherwise never.
async fn wait_for(ready: bool) {
    if !ready {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests;
