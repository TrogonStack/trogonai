use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn instruction(code: u32, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

pub(super) fn interrupt_then_read_packet(
    descriptor: RawFd,
    packet: OwnedFd,
) -> io::Result<JoinHandle<io::Result<usize>>> {
    let filter = [
        instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
        instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 3, libc::SYS_read as u32),
        instruction(
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            0,
            0,
            std::mem::offset_of!(libc::seccomp_data, args) as u32,
        ),
        instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 1, descriptor as u32),
        instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_USER_NOTIF),
        instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr().cast_mut(),
    };
    let listener = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_NEW_LISTENER,
            &program,
        )
    };
    if listener < 0 {
        return Err(io::Error::last_os_error());
    }
    let listener = unsafe { OwnedFd::from_raw_fd(listener as RawFd) };
    let reader = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
    std::thread::Builder::new().spawn(move || {
        let first = receive(&listener, reader, descriptor)?;
        respond(&listener, first.id, -libc::EINTR, 0)?;
        let next = receive(&listener, reader, descriptor)?;
        std::fs::File::from(packet).write_all(&[1, 2, 0, 0, 0])?;
        respond(&listener, next.id, 0, libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32)?;
        Ok(2)
    })
}

fn receive(listener: &OwnedFd, reader: u32, descriptor: RawFd) -> io::Result<libc::seccomp_notif> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "setup read notification did not arrive",
            ));
        }
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
        if request.pid != reader
            || request.data.nr != libc::SYS_read as i32
            || request.data.args[0] != descriptor as u64
            || request.data.args[2] != 5
        {
            return Err(io::Error::other("unexpected setup read notification"));
        }
        return Ok(request);
    }
}

fn respond(listener: &OwnedFd, id: u64, error: i32, flags: u32) -> io::Result<()> {
    let response = libc::seccomp_notif_resp {
        id,
        val: 0,
        error,
        flags,
    };
    if unsafe {
        libc::ioctl(
            listener.as_raw_fd(),
            libc::_IOWR::<libc::seccomp_notif_resp>(u32::from(b'!'), 1),
            &response,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
