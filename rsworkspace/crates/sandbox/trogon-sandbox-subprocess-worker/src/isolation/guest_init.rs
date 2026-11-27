//! The sandbox's PID 1. Bubblewrap's own init stays dumpable, so a payload
//! that shares its user could open `/proc/1/mem` for writing. This process
//! runs as PID 1 instead (`--as-pid-1`), turns dumpable off and starts the
//! payload as PID 2.
//! It ends with the payload and takes everything left in the sandbox with it.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;
use std::process::Command;

use anyhow::Context;
use clap::Parser;
use nix::fcntl::OFlag;
use nix::libc;

use super::relay::open_path;

pub(super) use super::constants::GUEST_INIT_BINARY;
pub use super::constants::INIT_EXIT_CODE;
pub use super::constants::INIT_FLAG;

/// The init binary, opened once so what gets mounted is what was checked.
pub(super) struct InitWiring {
    binary: OwnedFd,
}

impl InitWiring {
    pub(super) fn open(binary: &Path) -> anyhow::Result<Self> {
        let binary = open_path(binary, OFlag::empty())?;
        anyhow::ensure!(
            std::fs::metadata(format!("/proc/self/fd/{}", binary.as_raw_fd()))?.is_file(),
            "the init binary is not a regular file"
        );
        Ok(Self { binary })
    }

    pub(super) fn mount_args(&self) -> Vec<String> {
        vec![
            "--ro-bind-fd".to_string(),
            self.binary.as_raw_fd().to_string(),
            GUEST_INIT_BINARY.to_string(),
        ]
    }

    pub(super) fn inherited_fd(&self) -> i32 {
        self.binary.as_raw_fd()
    }

    /// Runs `binary args` as the child of the init.
    pub(super) fn wrap_command(binary: &str, args: &[String]) -> (String, Vec<String>) {
        let mut wrapped = vec![INIT_FLAG.to_string(), "--".to_string(), binary.to_string()];
        wrapped.extend(args.iter().cloned());
        (GUEST_INIT_BINARY.to_string(), wrapped)
    }
}

#[derive(Parser, Debug)]
#[command(name = "sandbox-subprocess-worker --guest-init")]
pub struct InitArgs {
    #[arg(long = "guest-init", required = true)]
    init: bool,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

pub fn is_init_invocation(args: &[std::ffi::OsString]) -> bool {
    args.get(1).is_some_and(|arg| arg == INIT_FLAG)
}

/// Runs inside the sandbox as its first process. Returns the exit code to
/// leave with.
pub fn run(args: InitArgs) -> anyhow::Result<i32> {
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    let (binary, rest) = args.command.split_first().context("no command to run")?;
    let payload = Command::new(binary)
        .args(rest)
        .spawn()
        .with_context(|| format!("starting {binary}"))?;
    let payload_pid = payload.id() as libc::pid_t;
    loop {
        let (pid, status) = wait_any()?;
        if pid == payload_pid {
            return Ok(exit_code(status));
        }
    }
}

fn wait_any() -> io::Result<(libc::pid_t, libc::c_int)> {
    loop {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid >= 0 {
            return Ok((pid, status));
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else {
        128 + libc::WTERMSIG(status)
    }
}

#[cfg(test)]
mod tests;
