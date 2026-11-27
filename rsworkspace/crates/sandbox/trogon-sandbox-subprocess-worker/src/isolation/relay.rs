//! The one way out of an empty network namespace: a relay inside the sandbox
//! that listens on loopback and forwards every connection to a unix socket
//! served by the lease's egress proxy on the host.
//!
//! The worker binary doubles as the relay (`--relay`). It runs as the
//! sandbox's first process, ahead of the payload, in this order:
//!
//! 1. bind the loopback listener,
//! 2. fork the relay process, which keeps the ability to create unix sockets,
//! 3. start the payload with the seccomp filters installed, which deny unix
//!    sockets to it and to everything it spawns.
//!
//! The filters are applied here and not by bubblewrap because bubblewrap
//! would also filter this process, and a relay that cannot create a unix
//! socket cannot reach the proxy.
//!
//! This process and the relay stay non-dumpable so the payload cannot read or
//! write their memory through `/proc`. If the payload kills either of them,
//! or the relay dies, the other one takes the payload down with it.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use clap::Parser;
use nix::fcntl::{OFlag, open};
use nix::libc;
use nix::sys::stat::Mode;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpStream, UnixStream};

use super::seccomp::{SeccompOptions, build_sandbox_programs};
pub use crate::relay_egress::RelayEgress;

pub(super) use super::constants::GUEST_RELAY_BINARY;
use super::constants::{ACCEPT_RETRY, GUEST_PROXY_SOCKET, RELAY_PROCESS_NAME};
pub use super::constants::{RELAY_EXIT_CODE, RELAY_FLAG, RELAY_PORT, RELAY_SOCKS_PORT};

pub fn relay_listen_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, RELAY_PORT))
}

pub fn relay_socks_listen_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, RELAY_SOCKS_PORT))
}

fn relay_listen_addrs() -> Vec<SocketAddr> {
    vec![relay_listen_addr(), relay_socks_listen_addr()]
}

/// The relay's host objects, opened once so what gets mounted is what was
/// checked and a path swapped afterwards cannot redirect the sandbox.
pub(super) struct RelayWiring {
    binary: OwnedFd,
    socket: OwnedFd,
}

impl RelayWiring {
    pub(super) fn open(egress: &RelayEgress) -> anyhow::Result<Self> {
        let binary = open_path(&egress.relay_binary, OFlag::empty())?;
        anyhow::ensure!(
            std::fs::metadata(proc_fd(&binary))?.is_file(),
            "relay binary {} is not a regular file",
            egress.relay_binary.display()
        );

        let socket = open_path(&egress.proxy_socket, OFlag::O_NOFOLLOW)?;
        let meta = std::fs::metadata(proc_fd(&socket))?;
        anyhow::ensure!(
            meta.file_type().is_socket(),
            "proxy socket {} is not a unix socket",
            egress.proxy_socket.display()
        );
        anyhow::ensure!(
            meta.uid() == nix::unistd::geteuid().as_raw() && meta.permissions().mode() & 0o077 == 0,
            "proxy socket {} must be reachable by its owner only",
            egress.proxy_socket.display()
        );
        Ok(Self { binary, socket })
    }

    pub(super) fn mount_args(&self) -> Vec<String> {
        vec![
            "--ro-bind-fd".to_string(),
            self.binary.as_raw_fd().to_string(),
            GUEST_RELAY_BINARY.to_string(),
            "--ro-bind-fd".to_string(),
            self.socket.as_raw_fd().to_string(),
            GUEST_PROXY_SOCKET.to_string(),
        ]
    }

    pub(super) fn inherited_fds(&self) -> [i32; 2] {
        [self.binary.as_raw_fd(), self.socket.as_raw_fd()]
    }

    /// Runs `binary args` behind the relay.
    pub(super) fn wrap_command(binary: &Path, args: &[String], seccomp: SeccompOptions) -> (String, Vec<String>) {
        let mut wrapped = vec![
            RELAY_FLAG.to_string(),
            "--socket".to_string(),
            GUEST_PROXY_SOCKET.to_string(),
        ];
        wrapped.extend(seccomp.flags());
        wrapped.push("--".to_string());
        wrapped.push(binary.to_string_lossy().into_owned());
        wrapped.extend(args.iter().cloned());
        (GUEST_RELAY_BINARY.to_string(), wrapped)
    }
}

pub(super) fn open_path(path: &Path, extra: OFlag) -> anyhow::Result<OwnedFd> {
    open(path, OFlag::O_PATH | OFlag::O_CLOEXEC | extra, Mode::empty())
        .with_context(|| format!("opening {}", path.display()))
}

fn proc_fd(fd: &OwnedFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))
}

#[derive(Parser, Debug)]
#[command(name = "sandbox-subprocess-worker --relay")]
pub struct RelayArgs {
    #[arg(long = "relay", required = true)]
    relay: bool,

    #[arg(long)]
    socket: PathBuf,

    #[arg(long = "listen", default_values_t = relay_listen_addrs())]
    listen: Vec<SocketAddr>,

    #[command(flatten)]
    seccomp: SeccompOptions,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

pub fn is_relay_invocation(args: &[std::ffi::OsString]) -> bool {
    args.get(1).is_some_and(|arg| arg == RELAY_FLAG)
}

/// Runs inside the sandbox. Returns the exit code of the process to exit with.
pub fn run(args: RelayArgs) -> anyhow::Result<i32> {
    let mut listeners = Vec::new();
    for listen in &args.listen {
        anyhow::ensure!(
            listen.ip().is_loopback(),
            "the relay may only listen on loopback, got {listen}"
        );
        listeners.push(TcpListener::bind(listen).with_context(|| format!("binding the relay listener on {listen}"))?);
        if let Some(v6) = ipv6_twin(*listen)
            && let Ok(twin) = TcpListener::bind(v6)
        {
            listeners.push(twin);
        }
    }
    for listener in &listeners {
        listener.set_nonblocking(true)?;
    }
    let programs = build_sandbox_programs(args.seccomp)?;

    match unsafe { nix::unistd::fork() }.context("forking the relay")? {
        nix::unistd::ForkResult::Child => {
            let code = match serve(listeners, args.socket) {
                Ok(()) => 0,
                Err(err) => {
                    super::diagnostics::to_stderr(format_args!("relay: {err:#}"));
                    1
                }
            };
            std::process::exit(code);
        }
        nix::unistd::ForkResult::Parent { child } => {
            drop(listeners);
            set_non_dumpable();
            supervise(child.as_raw(), &args.command, programs)
        }
    }
}

fn set_non_dumpable() {
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
}

fn ipv6_twin(listen: SocketAddr) -> Option<SocketAddr> {
    match listen {
        SocketAddr::V4(v4) if v4.ip().is_loopback() => {
            Some(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, v4.port(), 0, 0)))
        }
        _ => None,
    }
}

fn serve(listeners: Vec<TcpListener>, socket: PathBuf) -> anyhow::Result<()> {
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
        libc::prctl(libc::PR_SET_NAME, RELAY_PROCESS_NAME.as_ptr(), 0, 0, 0);
    }
    set_non_dumpable();
    if unsafe { libc::getppid() } == 1 {
        anyhow::bail!("the supervisor is already gone");
    }

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async move {
        let mut accepting = tokio::task::JoinSet::new();
        for listener in listeners {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            let socket = socket.clone();
            accepting.spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok((client, _)) => {
                            tokio::spawn(forward(client, socket.clone()));
                        }
                        Err(_) => tokio::time::sleep(ACCEPT_RETRY).await,
                    }
                }
            });
        }
        while accepting.join_next().await.is_some() {}
        Ok(())
    })
}

async fn forward(mut client: TcpStream, socket: PathBuf) {
    let Ok(mut proxy) = UnixStream::connect(&socket).await else {
        return;
    };
    let _ = copy_bidirectional(&mut client, &mut proxy).await;
}

fn supervise(relay: libc::pid_t, command: &[String], programs: Vec<seccompiler::BpfProgram>) -> anyhow::Result<i32> {
    let (binary, args) = command.split_first().context("no command to run")?;
    let mut payload = Command::new(binary);
    payload.args(args);
    unsafe {
        payload.pre_exec(move || {
            for program in &programs {
                seccompiler::apply_filter(program).map_err(io::Error::other)?;
            }
            Ok(())
        });
    }
    let payload = match payload.spawn() {
        Ok(child) => child,
        Err(err) => {
            kill_and_reap(relay);
            return Err(err).with_context(|| format!("starting {binary}"));
        }
    };
    let payload_pid = payload.id() as libc::pid_t;

    loop {
        let (pid, status) = wait_any()?;
        if pid == payload_pid {
            kill_and_reap(relay);
            return Ok(exit_code(status));
        }
        if pid == relay {
            super::diagnostics::to_stderr(format_args!("relay: the relay process died, ending the lease"));
            kill_and_reap(payload_pid);
            return Ok(RELAY_EXIT_CODE);
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

fn kill_and_reap(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
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
