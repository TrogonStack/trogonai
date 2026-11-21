use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub(super) struct Receipt {
    pub(super) child_lists: usize,
}

fn instruction(code: u32, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

pub(super) fn install(pid: i32, identity: OwnedFd) -> io::Result<Receiver<io::Result<Receipt>>> {
    let (send_listener, listener) = channel();
    let (send_receipt, receipt) = channel();
    let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
    // Service runtime closes, then retire outside Rust control flow before exit_group can stop this thread.
    std::thread::Builder::new().spawn(move || {
        let result = listener
            .recv()
            .map_err(io::Error::other)
            .and_then(|listener| observe(listener, tid, pid, identity, &send_receipt));
        if let Err(error) = result {
            let _ = send_receipt.send(Err(error));
        }
    })?;
    let filter = [
        instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
        instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            1,
            0,
            libc::SYS_close as u32,
        ),
        instruction(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            0,
            1,
            libc::SYS_exit_group as u32,
        ),
        instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_USER_NOTIF),
        instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr().cast_mut(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let raw = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_NEW_LISTENER,
            &program,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    send_listener.send(descriptor).map_err(io::Error::other)?;
    Ok(receipt)
}

fn observe(
    listener: OwnedFd,
    tid: u32,
    pid: i32,
    identity: OwnedFd,
    receipt: &std::sync::mpsc::Sender<io::Result<Receipt>>,
) -> io::Result<()> {
    let task_directory = std::path::PathBuf::from(format!("/proc/{pid}/task"));
    let mut child_lists = 0;
    let mut retired = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut poll = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 10) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
        if poll.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut request: libc::seccomp_notif = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::ioctl(
                listener.as_raw_fd(),
                libc::_IOWR::<libc::seccomp_notif>(u32::from(b'!'), 0),
                &mut request,
            )
        } < 0
        {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::EINTR | libc::ENOENT)) {
                continue;
            }
            return Err(error);
        }
        if request.pid != tid {
            return Err(io::Error::other("unexpected task-walk notification thread"));
        }
        if request.data.nr == libc::SYS_exit_group as i32 {
            if !retired {
                return Err(io::Error::other("worker exited before the task retirement oracle"));
            }
            release_exit_and_retire(&listener, request.id);
        }
        if request.data.nr != libc::SYS_close as i32 {
            return Err(io::Error::other("unexpected task-walk notification"));
        }
        let path = std::fs::read_link(format!("/proc/self/fd/{}", request.data.args[0]))?;
        if path.starts_with(&task_directory) && path.file_name() == Some(std::ffi::OsStr::new("children")) {
            child_lists += 1;
        }
        let finish = !retired && path == task_directory;
        if finish {
            if child_lists == 0 {
                return Err(io::Error::other(
                    "task directory closed before its child list was observed",
                ));
            }
            if unsafe { libc::kill(pid, libc::SIGKILL) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut poll = libc::pollfd {
                fd: identity.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 1000) } != 1 || poll.revents & libc::POLLIN == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "owned task parent did not exit",
                ));
            }
            retired = true;
        }
        let response = libc::seccomp_notif_resp {
            id: request.id,
            val: 0,
            error: 0,
            flags: libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32,
        };
        if unsafe {
            libc::ioctl(
                listener.as_raw_fd(),
                libc::_IOWR::<libc::seccomp_notif_resp>(u32::from(b'!'), 1),
                &response,
            )
        } < 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                continue;
            }
            return Err(error);
        }
        if finish {
            let _ = receipt.send(Ok(Receipt { child_lists }));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "task-walk observer outlived its isolated worker",
    ))
}

fn release_exit_and_retire(listener: &OwnedFd, id: u64) -> ! {
    let response = libc::seccomp_notif_resp {
        id,
        val: 0,
        error: 0,
        flags: libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32,
    };
    // A failed response leaves the main thread blocked and the worker deadline rejects the test.
    unsafe {
        libc::ioctl(
            listener.as_raw_fd(),
            libc::_IOWR::<libc::seccomp_notif_resp>(u32::from(b'!'), 1),
            &response,
        );
        libc::syscall(libc::SYS_exit, 0);
        libc::abort();
    }
}
