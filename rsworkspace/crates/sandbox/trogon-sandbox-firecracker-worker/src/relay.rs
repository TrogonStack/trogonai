//! The process that holds Firecracker inside a lease's network namespace.
//!
//! The lease's egress proxy listens on a unix socket in the host namespace, which the guest
//! cannot reach over its tap. The relay is started inside the namespace by the root helper,
//! already dropped to the worker's user, binds the one address the namespace's firewall lets
//! the guest reach, and forwards each connection to the proxy's socket. It then runs
//! Firecracker as its child and exits with it, so stopping the lease is killing the relay.

use std::ffi::OsString;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::net::{TcpListener, UnixStream};
use tokio::process::Command;

#[derive(Debug, Eq, PartialEq)]
pub struct RelayArgs {
    pub listen: SocketAddr,
    pub socket: PathBuf,
    pub pid_file: PathBuf,
    pub program: OsString,
    pub program_args: Vec<OsString>,
}

impl RelayArgs {
    /// `--listen ADDR --socket PATH --pid-file PATH -- PROGRAM [ARGS...]`
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut args = args.into_iter();
        let mut listen = None;
        let mut socket = None;
        let mut pid_file = None;
        loop {
            let flag = args.next().ok_or("missing `-- PROGRAM`")?;
            if flag == "--" {
                break;
            }
            let value = args
                .next()
                .ok_or_else(|| format!("{} needs a value", flag.to_string_lossy()))?;
            match flag.to_str() {
                Some("--listen") => {
                    listen = Some(
                        value
                            .to_str()
                            .and_then(|v| v.parse().ok())
                            .ok_or("--listen needs ADDR:PORT")?,
                    );
                }
                Some("--socket") => socket = Some(PathBuf::from(value)),
                Some("--pid-file") => pid_file = Some(PathBuf::from(value)),
                _ => return Err(format!("unknown flag {}", flag.to_string_lossy())),
            }
        }
        Ok(Self {
            listen: listen.ok_or("--listen is required")?,
            socket: socket.ok_or("--socket is required")?,
            pid_file: pid_file.ok_or("--pid-file is required")?,
            program: args.next().ok_or("missing PROGRAM after --")?,
            program_args: args.collect(),
        })
    }
}

/// Writes one diagnostic line to stderr, which the root helper that started the relay collects.
fn report(message: std::fmt::Arguments<'_>) {
    let _ = writeln!(std::io::stderr(), "{RELAY_SUBCOMMAND}: {message}");
}

/// Runs the relay to completion and returns the process exit code.
pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = match RelayArgs::parse(args) {
        Ok(args) => args,
        Err(e) => {
            report(format_args!("{e}"));
            return 2;
        }
    };
    // One thread, so the death signal armed on the child follows this process's lifetime
    // rather than that of whichever runtime thread happened to spawn it.
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            report(format_args!("starting the runtime: {e}"));
            return 1;
        }
    };
    match runtime.block_on(serve(args)) {
        Ok(code) => code,
        Err(e) => {
            report(format_args!("{e}"));
            1
        }
    }
}

async fn serve(args: RelayArgs) -> std::io::Result<i32> {
    let listener = TcpListener::bind(args.listen).await?;
    relay(listener, args).await
}

async fn relay(listener: TcpListener, args: RelayArgs) -> std::io::Result<i32> {
    tokio::fs::write(&args.pid_file, std::process::id().to_string()).await?;

    let mut command = Command::new(&args.program);
    command.args(&args.program_args).stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe and touches no memory of the parent.
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;

    loop {
        tokio::select! {
            status = child.wait() => {
                let status = status?;
                return Ok(status.code().unwrap_or(1));
            }
            accepted = listener.accept() => {
                let Ok((mut inbound, _)) = accepted else { continue };
                let socket = args.socket.clone();
                tokio::spawn(async move {
                    match UnixStream::connect(&socket).await {
                        Ok(mut outbound) => {
                            let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                        }
                        Err(e) => report(format_args!("connecting to the egress proxy: {e}")),
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;

mod constants;
pub use constants::RELAY_SUBCOMMAND;
