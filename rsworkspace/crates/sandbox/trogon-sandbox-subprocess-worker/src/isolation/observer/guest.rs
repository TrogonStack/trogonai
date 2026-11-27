use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use clap::Parser;
use nix::libc;
use seccompiler::{BpfProgram, sock_filter};

use super::trap::{self, PATH_MAX, Tracee};
use super::{OBSERVE_FLAG, WireEvent};
use crate::isolation::seccomp::{SeccompOptions, build_sandbox_programs};

use super::constants::{
    PAGE, POLL_INTERVAL_MS, SECCOMP_FILTER_FLAG_NEW_LISTENER, SECCOMP_SET_MODE_FILTER, SECCOMP_USER_NOTIF_FLAG_CONTINUE,
};

#[repr(C)]
struct SeccompData {
    nr: i32,
    arch: u32,
    instruction_pointer: u64,
    args: [u64; 6],
}

#[repr(C)]
struct Notification {
    id: u64,
    pid: u32,
    flags: u32,
    data: SeccompData,
}

#[repr(C)]
struct Response {
    id: u64,
    val: i64,
    error: i32,
    flags: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const sock_filter,
}

mod ioctls;

#[derive(Parser, Debug)]
#[command(name = "sandbox-subprocess-worker --observe")]
pub struct ObserveArgs {
    #[arg(long = "observe", required = true)]
    observe: bool,

    #[arg(long)]
    socket: PathBuf,

    /// Install the sandbox seccomp filters on the payload, as bubblewrap
    /// would have, so this process can still create the event socket first.
    #[arg(long)]
    pub(super) apply_filters: bool,

    #[command(flatten)]
    pub(super) seccomp: SeccompOptions,

    #[arg(last = true, required = true)]
    pub(super) command: Vec<String>,
}

pub fn is_observe_invocation(args: &[std::ffi::OsString]) -> bool {
    args.get(1).is_some_and(|arg| arg == OBSERVE_FLAG)
}

/// Runs inside the sandbox as its first process. The payload starts with a
/// user-notification filter installed; this process answers every
/// notification with "carry on" after it has told the host what the call was
/// about to do. If the kernel cannot do that, or the host is not listening,
/// the payload runs unobserved.
pub fn run(args: ObserveArgs) -> anyhow::Result<i32> {
    let events = connect(&args.socket);
    let filters = if args.apply_filters {
        build_sandbox_programs(args.seccomp)?
    } else {
        Vec::new()
    };
    let observe = events.as_ref().map(|_| trap::observe_program());

    let (binary, rest) = args.command.split_first().context("no command to run")?;
    let (supervisor_end, payload_end) = UnixStream::pair().context("creating the handoff pair")?;
    let payload_fd = payload_end.as_raw_fd();
    let mut payload = Command::new(binary);
    payload.args(rest);
    unsafe {
        payload.pre_exec(move || {
            for program in &filters {
                seccompiler::apply_filter(program).map_err(io::Error::other)?;
            }
            if let Some(Ok(listener)) = observe.as_ref().map(install_listener) {
                let sent = send_fd(payload_fd, listener);
                libc::close(listener);
                sent?;
            }
            Ok(())
        });
    }
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    let mut child = payload.spawn().with_context(|| format!("starting {binary}"))?;
    drop(payload_end);

    let stop = Arc::new(AtomicBool::new(false));
    let supervisor = match (receive_fd(supervisor_end.as_raw_fd()), events) {
        (Some(listener), Some(events)) => {
            let stop = Arc::clone(&stop);
            Some(std::thread::spawn(move || supervise(listener, events, stop)))
        }
        _ => None,
    };

    let status = child.wait().context("waiting for the payload")?;
    stop.store(true, Ordering::Relaxed);
    if let Some(supervisor) = supervisor {
        let _ = supervisor.join();
    }
    Ok(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

fn connect(socket: &PathBuf) -> Option<UnixDatagram> {
    let events = UnixDatagram::unbound().ok()?;
    events.connect(socket).ok()?;
    events.set_nonblocking(true).ok()?;
    Some(events)
}

fn install_listener(program: &BpfProgram) -> io::Result<RawFd> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let fprog = SockFprog {
        len: u16::try_from(program.len()).map_err(io::Error::other)?,
        filter: program.as_ptr(),
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            SECCOMP_FILTER_FLAG_NEW_LISTENER,
            &fprog as *const SockFprog,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(fd as RawFd)
    }
}

fn zeroed_message() -> libc::msghdr {
    unsafe { std::mem::zeroed() }
}

fn send_fd(socket: RawFd, fd: RawFd) -> io::Result<()> {
    let mut byte = 0u8;
    let mut iov = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    let mut control = [0u64; 8];
    let mut message = zeroed_message();
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as _;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as _;
        std::ptr::copy_nonoverlapping(
            (&fd as *const RawFd).cast::<u8>(),
            libc::CMSG_DATA(header),
            size_of::<RawFd>(),
        );
    }
    if unsafe { libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn receive_fd(socket: RawFd) -> Option<OwnedFd> {
    let mut byte = 0u8;
    let mut iov = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    let mut control = [0u64; 8];
    let mut message = zeroed_message();
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as _;
    loop {
        let received = unsafe { libc::recvmsg(socket, &mut message, libc::MSG_CMSG_CLOEXEC) };
        if received >= 0 {
            break;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return None;
        }
    }
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() || (*header).cmsg_level != libc::SOL_SOCKET || (*header).cmsg_type != libc::SCM_RIGHTS {
            return None;
        }
        let mut fd: RawFd = -1;
        std::ptr::copy_nonoverlapping(
            libc::CMSG_DATA(header),
            (&mut fd as *mut RawFd).cast::<u8>(),
            size_of::<RawFd>(),
        );
        (fd >= 0).then(|| OwnedFd::from_raw_fd(fd))
    }
}

/// Answers notifications until told to stop. It never denies a call and
/// never panics: dropping the listener early would make every trapped call
/// fail with `ENOSYS`.
fn supervise(listener: OwnedFd, events: UnixDatagram, stop: Arc<AtomicBool>) {
    let fd = listener.as_raw_fd();
    while !stop.load(Ordering::Relaxed) {
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, POLL_INTERVAL_MS) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if ready == 0 {
            continue;
        }
        if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return;
        }
        if poll.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut notification: Notification = unsafe { std::mem::zeroed() };
        if unsafe { ioctls::receive(fd, &mut notification) }.is_err() {
            continue;
        }
        let process = Process { pid: notification.pid };
        let observed = trap::describe(i64::from(notification.data.nr), &notification.data.args, &process);
        let still_valid = unsafe { ioctls::id_valid(fd, &notification.id) }.is_ok();
        if still_valid {
            for event in observed {
                emit(&events, event.operation, event.path);
            }
        }
        let mut response = Response {
            id: notification.id,
            val: 0,
            error: 0,
            flags: SECCOMP_USER_NOTIF_FLAG_CONTINUE,
        };
        let _ = unsafe { ioctls::send(fd, &mut response) };
    }
}

fn emit(events: &UnixDatagram, operation: trogon_sandbox_contract::FsOperation, path: String) {
    let Ok(payload) = serde_json::to_vec(&WireEvent { operation, path }) else {
        return;
    };
    unsafe {
        libc::send(
            events.as_raw_fd(),
            payload.as_ptr().cast(),
            payload.len(),
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
        );
    }
}

struct Process {
    pid: u32,
}

impl Process {
    fn read(&self, address: u64, buffer: &mut [u8]) -> Option<usize> {
        let local = libc::iovec {
            iov_base: buffer.as_mut_ptr().cast(),
            iov_len: buffer.len(),
        };
        let remote = libc::iovec {
            iov_base: address as usize as *mut libc::c_void,
            iov_len: buffer.len(),
        };
        let read = unsafe { libc::process_vm_readv(self.pid as libc::pid_t, &local, 1, &remote, 1, 0) };
        usize::try_from(read).ok().filter(|read| *read > 0)
    }
}

impl Tracee for Process {
    fn read_c_string(&self, address: u64) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut at = address;
        while out.len() < PATH_MAX {
            let until_page_end = (PAGE - at % PAGE) as usize;
            let mut chunk = vec![0u8; until_page_end.min(PATH_MAX - out.len())];
            let read = self.read(at, &mut chunk)?;
            if let Some(end) = chunk[..read].iter().position(|byte| *byte == 0) {
                out.extend_from_slice(&chunk[..end]);
                return Some(out);
            }
            out.extend_from_slice(&chunk[..read]);
            at += read as u64;
        }
        None
    }

    fn read_u64(&self, address: u64) -> Option<u64> {
        let mut bytes = [0u8; 8];
        (self.read(address, &mut bytes)? == bytes.len()).then(|| u64::from_ne_bytes(bytes))
    }

    fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.pid)).ok()
    }

    fn fd_path(&self, fd: i32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/fd/{fd}", self.pid)).ok()
    }
}
