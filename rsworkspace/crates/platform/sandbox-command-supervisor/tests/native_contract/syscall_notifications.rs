use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

pub(super) enum Syscall {
    Session,
    Fork,
}

pub(super) enum Action {
    Continue,
    Refuse(i32),
}

enum Refusals {
    Repeated,
    Once,
    Finished,
}

pub(super) struct Policy {
    allowed: usize,
    refusals: Refusals,
    errno: i32,
}

impl Policy {
    pub(super) fn reject_after(allowed: usize, errno: i32) -> Self {
        Self {
            allowed,
            refusals: Refusals::Repeated,
            errno,
        }
    }

    pub(super) fn reject_once_after(allowed: usize, errno: i32) -> Self {
        Self {
            allowed,
            refusals: Refusals::Once,
            errno,
        }
    }

    fn next(&mut self) -> Action {
        if self.allowed > 0 {
            self.allowed -= 1;
            return Action::Continue;
        }
        match self.refusals {
            Refusals::Repeated => Action::Refuse(self.errno),
            Refusals::Once => {
                self.refusals = Refusals::Finished;
                Action::Refuse(self.errno)
            }
            Refusals::Finished => Action::Continue,
        }
    }
}

pub(super) struct Observation {
    pub(super) tid: u32,
    pub(super) action: Action,
    pub(super) process: Option<OwnedFd>,
}

pub(super) struct Report {
    pub(super) first: Vec<Observation>,
    pub(super) continued: usize,
    pub(super) refused: usize,
}

pub(super) struct PendingListener {
    socket: UnixDatagram,
    policy: Policy,
}

impl PendingListener {
    pub(super) fn install(command: &mut Command, syscall: Syscall, policy: Policy) -> io::Result<Self> {
        let (receiver, sender) = UnixDatagram::pair()?;
        receiver.set_read_timeout(Some(super::WAIT))?;
        let number = match syscall {
            Syscall::Session => libc::SYS_setsid,
            Syscall::Fork => libc::SYS_clone,
        };
        let filter = match syscall {
            Syscall::Session => vec![
                instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
                instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 1, number as u32),
                instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_USER_NOTIF),
                instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
            ],
            Syscall::Fork => vec![
                instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
                instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 4, number as u32),
                instruction(
                    libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
                    0,
                    0,
                    std::mem::offset_of!(libc::seccomp_data, args) as u32,
                ),
                instruction(
                    libc::BPF_ALU | libc::BPF_AND | libc::BPF_K,
                    0,
                    0,
                    libc::CLONE_THREAD as u32,
                ),
                instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 1, 0),
                instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_USER_NOTIF),
                instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
            ],
        };
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let program = libc::sock_fprog {
                    len: filter.len() as u16,
                    filter: filter.as_ptr().cast_mut(),
                };
                let listener = libc::syscall(
                    libc::SYS_seccomp,
                    libc::SECCOMP_SET_MODE_FILTER,
                    libc::SECCOMP_FILTER_FLAG_NEW_LISTENER,
                    &program,
                );
                if listener < 0 {
                    return Err(io::Error::last_os_error());
                }
                let result = send_listener(sender.as_raw_fd(), listener as RawFd);
                libc::close(listener as RawFd);
                result
            });
        }
        Ok(Self {
            socket: receiver,
            policy,
        })
    }

    pub(super) fn start(self) -> io::Result<Listener> {
        let descriptor = receive_listener(&self.socket)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = std::thread::spawn(move || observe(descriptor, self.policy, &worker_stop));
        Ok(Listener {
            stop,
            worker: Some(worker),
        })
    }
}

pub(super) struct Listener {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<Report>>>,
}

impl Listener {
    pub(super) fn finish(mut self) -> io::Result<Report> {
        self.stop.store(true, Ordering::Release);
        let worker = self
            .worker
            .take()
            .ok_or_else(|| io::Error::other("missing notification worker"))?;
        match worker.join() {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn instruction(code: u32, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

fn send_listener(socket: RawFd, listener: RawFd) -> io::Result<()> {
    let mut byte = 1_u8;
    let mut vector = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    let mut control = [0_usize; 4];
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as usize;
        std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<RawFd>(), listener);
        if libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL) != 1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn receive_listener(socket: &UnixDatagram) -> io::Result<OwnedFd> {
    let mut byte = 0_u8;
    let mut vector = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    let mut control = [0_usize; 4];
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = size_of::<[usize; 4]>();
    let received = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if received != 1 || byte != 1 || message.msg_flags & libc::MSG_CTRUNC != 0 || header.is_null() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    unsafe {
        if (*header).cmsg_level != libc::SOL_SOCKET
            || (*header).cmsg_type != libc::SCM_RIGHTS
            || (*header).cmsg_len != libc::CMSG_LEN(size_of::<RawFd>() as u32) as usize
        {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let descriptor = std::ptr::read_unaligned(libc::CMSG_DATA(header).cast::<RawFd>());
        if descriptor < 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        Ok(OwnedFd::from_raw_fd(descriptor))
    }
}

fn observe(descriptor: OwnedFd, mut policy: Policy, stop: &AtomicBool) -> io::Result<Report> {
    let mut report = Report {
        first: Vec::new(),
        continued: 0,
        refused: 0,
    };
    while !stop.load(Ordering::Acquire) {
        let mut poll = libc::pollfd {
            fd: descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 20) } < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if poll.revents & libc::POLLHUP != 0 {
            break;
        }
        if poll.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut request: libc::seccomp_notif = unsafe { zeroed() };
        if unsafe {
            libc::ioctl(
                descriptor.as_raw_fd(),
                libc::_IOWR::<libc::seccomp_notif>(u32::from(b'!'), 0),
                &mut request,
            )
        } < 0
        {
            match io::Error::last_os_error().raw_os_error() {
                Some(libc::EINTR | libc::ENOENT) => continue,
                _ => return Err(io::Error::last_os_error()),
            }
        }
        let action = policy.next();
        let process = match (&action, &policy.refusals) {
            (Action::Refuse(_), Refusals::Repeated) => {
                let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, request.pid as i32, 0) };
                if raw < 0 {
                    return Err(io::Error::last_os_error());
                }
                Some(unsafe { OwnedFd::from_raw_fd(raw as RawFd) })
            }
            _ => None,
        };
        let mut response = libc::seccomp_notif_resp {
            id: request.id,
            val: 0,
            error: 0,
            flags: 0,
        };
        match action {
            Action::Continue => response.flags = libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32,
            Action::Refuse(errno) => response.error = -errno,
        }
        if unsafe {
            libc::ioctl(
                descriptor.as_raw_fd(),
                libc::_IOWR::<libc::seccomp_notif_resp>(u32::from(b'!'), 1),
                &mut response,
            )
        } < 0
        {
            if io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        match action {
            Action::Continue => report.continued += 1,
            Action::Refuse(_) => report.refused += 1,
        }
        if report.first.len() < 3 {
            report.first.push(Observation {
                tid: request.pid,
                action,
                process,
            });
        }
    }
    Ok(report)
}
