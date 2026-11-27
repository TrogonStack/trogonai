//! The nested PID namespace a payload runs in. The worker binary doubles as
//! the helper that sets it up (`--nested`) and runs as the last step before
//! the payload, so the relay, the observer and the init stay outside the namespace and the payload
//! cannot see them, whatever capabilities it holds:
//!
//! 1. unshare a PID and a mount namespace, through a user namespace of its own
//!    when bubblewrap's holds no `CAP_SYS_ADMIN`,
//! 2. fork a stub that stays in bubblewrap's PID namespace, forwards signals
//!    and relays the exit status, and a PID 1 in the new namespace,
//! 3. in the new namespace mount a fresh `/proc`, allow no user namespace
//!    beneath this one, clear the ambient capabilities and fork the worker,
//! 4. the worker installs the seccomp filters and starts the payload.
//!
//! In a user namespace made here a root caller's payload ends with the full
//! capability set.

use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::Ordering;

use anyhow::Context;
use clap::Parser;
use nix::libc;

use super::guest_init::GUEST_INIT_BINARY;
use super::seccomp::{SeccompOptions, build_sandbox_programs};

pub use super::constants::NESTED_EXIT_CODE;
pub use super::constants::NESTED_FLAG;
use super::constants::{FORWARD_TARGET, FORWARDED_SIGNALS, NEW_PID_AND_MOUNT};

/// Runs `binary args` in the nested namespace.
pub(super) fn wrap_command(binary: &str, args: &[String], seccomp: SeccompOptions) -> (String, Vec<String>) {
    let mut wrapped = vec![NESTED_FLAG.to_string()];
    wrapped.extend(seccomp.flags());
    wrapped.push("--".to_string());
    wrapped.push(binary.to_string());
    wrapped.extend(args.iter().cloned());
    (GUEST_INIT_BINARY.to_string(), wrapped)
}

#[derive(Parser, Debug)]
#[command(name = "sandbox-subprocess-worker --nested")]
pub struct NestedArgs {
    #[arg(long = "nested", required = true)]
    nested: bool,

    #[command(flatten)]
    seccomp: SeccompOptions,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

pub fn is_nested_invocation(args: &[std::ffi::OsString]) -> bool {
    args.get(1).is_some_and(|arg| arg == NESTED_FLAG)
}

/// Runs inside the sandbox, ahead of the payload. Returns the exit code to
/// leave with.
pub fn run(args: NestedArgs) -> anyhow::Result<i32> {
    let payload_filters = build_sandbox_programs(args.seccomp)?;
    let stub_filters = if args.seccomp.allow_nested_namespaces {
        Vec::new()
    } else {
        build_sandbox_programs(SeccompOptions {
            allow_unix_sockets: true,
            allow_nested_namespaces: false,
        })?
    };
    let made_user_namespace = enter_namespaces()?;
    set_non_dumpable()?;

    match unsafe { nix::unistd::fork() }.context("forking the nested init")? {
        nix::unistd::ForkResult::Parent { child } => stub(child.as_raw(), &stub_filters),
        nix::unistd::ForkResult::Child => init(args, payload_filters, made_user_namespace),
    }
}

/// Whether a user namespace had to be made for the unshare.
fn enter_namespaces() -> anyhow::Result<bool> {
    match unshare(NEW_PID_AND_MOUNT) {
        Ok(()) => return Ok(false),
        Err(err) if err.raw_os_error() == Some(libc::EPERM) => {}
        Err(err) => return Err(err).context("unshare(CLONE_NEWPID|CLONE_NEWNS)"),
    }

    let uid = nix::unistd::geteuid().as_raw();
    let gid = nix::unistd::getegid().as_raw();
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 1, 0, 0, 0) };
    unshare(libc::CLONE_NEWUSER).context("unshare(CLONE_NEWUSER)")?;
    write_file("/proc/self/setgroups", "deny").context(
        "write /proc/self/setgroups (nested userns is capability-restricted; caller must provide CAP_SYS_ADMIN)",
    )?;
    write_file("/proc/self/uid_map", &format!("{uid} {uid} 1\n")).context("write /proc/self/uid_map")?;
    write_file("/proc/self/gid_map", &format!("{gid} {gid} 1\n")).context("write /proc/self/gid_map")?;
    unshare(NEW_PID_AND_MOUNT).context("unshare(CLONE_NEWPID|CLONE_NEWNS) after userns")?;
    Ok(true)
}

fn unshare(flags: libc::c_int) -> io::Result<()> {
    if unsafe { libc::unshare(flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn write_file(path: &str, text: &str) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.write_all(text.as_bytes())
}

fn set_non_dumpable() -> anyhow::Result<()> {
    anyhow::ensure!(
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } == 0,
        "prctl(PR_SET_DUMPABLE): {}",
        io::Error::last_os_error()
    );
    Ok(())
}

/// The half that stays in bubblewrap's PID namespace. It takes the
/// namespaces filter, not the socket one, so code it was made to run is no
/// better placed than the payload.
fn stub(init: libc::pid_t, filters: &[seccompiler::BpfProgram]) -> anyhow::Result<i32> {
    for program in filters {
        seccompiler::apply_filter(program).map_err(io::Error::other)?;
    }
    forward_signals_to(init);
    wait_for(init)
}

/// PID 1 of the new namespace.
fn init(args: NestedArgs, filters: Vec<seccompiler::BpfProgram>, made_user_namespace: bool) -> anyhow::Result<i32> {
    set_non_dumpable()?;
    mount("", "/", "", libc::MS_REC | libc::MS_PRIVATE).context("mount(MS_PRIVATE)")?;
    // EPERM means a masked /proc lies underneath (an unprivileged container);
    // the payload then still sees the outer one.
    match mount(
        "proc",
        "/proc",
        "proc",
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
    ) {
        Err(err) if err.raw_os_error() != Some(libc::EPERM) => {
            return Err(err).context("mount(/proc)");
        }
        _ => {}
    }
    // Not fatal: where /proc could not be mounted, /proc/sys is commonly
    // read-only and the filter refuses the same calls.
    if !args.seccomp.allow_nested_namespaces && made_user_namespace {
        let _ = write_file("/proc/sys/user/max_user_namespaces", "0");
    }
    anyhow::ensure!(
        unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0,) } == 0,
        "prctl(PR_CAP_AMBIENT_CLEAR_ALL): {}",
        io::Error::last_os_error()
    );

    match unsafe { nix::unistd::fork() }.context("forking the payload")? {
        nix::unistd::ForkResult::Parent { child } => {
            forward_signals_to(child.as_raw());
            wait_for(child.as_raw())
        }
        nix::unistd::ForkResult::Child => {
            let (binary, rest) = args.command.split_first().context("no command to run")?;
            for program in &filters {
                seccompiler::apply_filter(program).map_err(io::Error::other)?;
            }
            let err = Command::new(binary).args(rest).exec();
            Err(err).with_context(|| format!("starting {binary}"))
        }
    }
}

fn mount(source: &str, target: &str, fstype: &str, flags: libc::c_ulong) -> io::Result<()> {
    let text = |value: &str| CString::new(value).map_err(io::Error::other);
    let (source, target, fstype) = (text(source)?, text(target)?, text(fstype)?);
    let pointer = |value: &CString| {
        if value.as_bytes().is_empty() {
            std::ptr::null()
        } else {
            value.as_ptr()
        }
    };
    let rc = unsafe {
        libc::mount(
            pointer(&source),
            target.as_ptr(),
            pointer(&fstype),
            flags,
            std::ptr::null(),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

extern "C" fn forward_signal(signal: libc::c_int) {
    let target = FORWARD_TARGET.load(Ordering::Relaxed);
    if target > 0 {
        unsafe { libc::kill(target, signal) };
    }
}

/// PID 1 drops the signals it has no handler for, so both halves install one.
fn forward_signals_to(target: libc::pid_t) {
    FORWARD_TARGET.store(target, Ordering::Relaxed);
    for signal in FORWARDED_SIGNALS {
        unsafe { libc::signal(signal, forward_signal as *const () as libc::sighandler_t) };
    }
}

/// Waits for `main`, reaping whatever else ends first.
fn wait_for(main: libc::pid_t) -> anyhow::Result<i32> {
    loop {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err).context("waiting for the payload");
        }
        if pid == main {
            return Ok(exit_code(status));
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
