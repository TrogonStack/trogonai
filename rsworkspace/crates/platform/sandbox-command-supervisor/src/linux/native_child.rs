use super::*;
use std::os::unix::fs::PermissionsExt;

pub(super) struct Prepared {
    pub(super) argv: Vec<CString>,
    pub(super) environment: Vec<CString>,
    pub(super) executable: CString,
    pub(super) cwd: Option<CString>,
    pub(super) timeout: Option<Duration>,
    pub(super) release_timeout: Duration,
}

impl Prepared {
    pub(super) fn from_command(command: Command) -> Result<Self, ErrorCode> {
        let Command::Init {
            argv,
            env,
            cwd,
            timeout_ms,
            release_timeout_ms,
            ..
        } = command
        else {
            return Err(ErrorCode::InvalidCommand);
        };
        let argv = argv.into_inner();
        let env = env.into_inner();
        let cwd = cwd.map(crate::WorkingDirectory::into_inner);
        let timeout_ms = timeout_ms.map(|timeout| timeout.get());
        let release_timeout_ms = release_timeout_ms.map(|timeout| timeout.get());
        let current = std::env::current_dir().map_err(|_| ErrorCode::SetupFailed)?;
        let base = cwd
            .as_deref()
            .map(Path::new)
            .map(|path| {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    current.join(path)
                }
            })
            .unwrap_or(current);
        let command = if argv[0].contains('/') {
            argv[0].clone()
        } else {
            let path = env
                .get("PATH")
                .map(String::as_str)
                .unwrap_or("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
            path.split(':')
                .map(|directory| {
                    let directory = if directory.is_empty() {
                        base.clone()
                    } else if Path::new(directory).is_absolute() {
                        Path::new(directory).to_path_buf()
                    } else {
                        base.join(directory)
                    };
                    directory.join(&argv[0])
                })
                .find(|path| {
                    std::fs::metadata(path)
                        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                })
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| argv[0].clone())
        };
        Ok(Self {
            argv: argv
                .into_iter()
                .map(CString::new)
                .collect::<Result<_, _>>()
                .map_err(|_| ErrorCode::InvalidCommand)?,
            environment: env
                .into_iter()
                .map(|(key, value)| CString::new(format!("{key}={value}")))
                .collect::<Result<_, _>>()
                .map_err(|_| ErrorCode::InvalidCommand)?,
            executable: CString::new(command).map_err(|_| ErrorCode::InvalidCommand)?,
            cwd: cwd
                .map(CString::new)
                .transpose()
                .map_err(|_| ErrorCode::InvalidCommand)?,
            timeout: timeout_ms.map(Duration::from_millis),
            release_timeout: Duration::from_millis(release_timeout_ms.unwrap_or(DEFAULT_RELEASE_TIMEOUT_MS)),
        })
    }
}

pub(super) struct Root {
    pub(super) pid: i32,
    pub(super) pidfd: OwnedFd,
    pub(super) identity: Identity,
    pub(super) gate: Option<OwnedFd>,
    pub(super) setup: OwnedFd,
    pub(super) stdout: Option<OwnedFd>,
    pub(super) stderr: Option<OwnedFd>,
    pub(super) status: Option<RootStatus>,
    pub(super) reaped: bool,
}

unsafe fn child_error(descriptor: RawFd, error: i32) -> ! {
    let mut message = [0; 5];
    message[0] = 1;
    message[1..].copy_from_slice(&error.to_le_bytes());
    unsafe {
        libc::write(descriptor, message.as_ptr().cast(), message.len());
        libc::_exit(if error == libc::ENOENT { 127 } else { 126 });
    }
}

struct Launch {
    gate_read: OwnedFd,
    gate_write: OwnedFd,
    out_read: OwnedFd,
    out_write: OwnedFd,
    err_read: OwnedFd,
    err_write: OwnedFd,
    setup_read: OwnedFd,
    setup_write: OwnedFd,
    null: OwnedFd,
    argv: Vec<*const libc::c_char>,
    environment: Vec<*const libc::c_char>,
}

pub(super) fn spawn(prepared: &Prepared) -> Result<Root, ErrorCode> {
    let (gate_read, gate_write) = pipe()?;
    let (out_read, out_write) = pipe()?;
    let (err_read, err_write) = pipe()?;
    let (setup_read, setup_write) = pipe()?;
    let null = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if null < 0 {
        return Err(ErrorCode::SetupFailed);
    }
    let null = unsafe { OwnedFd::from_raw_fd(null) };
    let argv: Vec<_> = prepared
        .argv
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let environment: Vec<_> = prepared
        .environment
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(ErrorCode::SetupFailed);
    }
    after_fork(
        pid,
        prepared,
        Launch {
            gate_read,
            gate_write,
            out_read,
            out_write,
            err_read,
            err_write,
            setup_read,
            setup_write,
            null,
            argv,
            environment,
        },
    )
}

fn after_fork(pid: i32, prepared: &Prepared, launch: Launch) -> Result<Root, ErrorCode> {
    let Launch {
        gate_read,
        gate_write,
        out_read,
        out_write,
        err_read,
        err_write,
        setup_read,
        setup_write,
        null,
        argv,
        environment,
    } = launch;
    if pid == 0 {
        unsafe {
            // Duplicate away from 0..4 before replacing any inherited descriptor.
            let gate = libc::fcntl(gate_read.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10);
            let setup = libc::fcntl(setup_write.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10);
            if gate < 0 || setup < 0 {
                child_error(setup_write.as_raw_fd(), errno());
            }
            if libc::dup2(null.as_raw_fd(), 0) < 0
                || libc::dup2(out_write.as_raw_fd(), 1) < 0
                || libc::dup2(err_write.as_raw_fd(), 2) < 0
                || libc::dup2(gate, 3) < 0
                || libc::dup2(setup, 4) < 0
            {
                child_error(setup, errno());
            }
            libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(4, libc::F_SETFD, libc::FD_CLOEXEC);
            if libc::syscall(libc::SYS_close_range, 5_u32, u32::MAX, 0) < 0 {
                child_error(4, errno());
            }
            for signal in [
                libc::SIGCHLD,
                libc::SIGPIPE,
                libc::SIGTERM,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGHUP,
            ] {
                if disposition(signal, libc::SIG_DFL).is_err() {
                    child_error(4, errno());
                }
            }
            if libc::setsid() < 0 {
                child_error(4, errno());
            }
            if let Some(directory) = &prepared.cwd
                && libc::chdir(directory.as_ptr()) < 0
            {
                child_error(4, errno());
            }
            let ready = [0_u8];
            if libc::write(4, ready.as_ptr().cast(), 1) != 1 {
                child_error(4, errno());
            }
            let mut release = [0_u8];
            loop {
                let read = libc::read(3, release.as_mut_ptr().cast(), 1);
                if read < 0 && errno() == libc::EINTR {
                    continue;
                }
                if read != 1 || release[0] != b'R' {
                    libc::_exit(125);
                }
                break;
            }
            libc::close(3);
            libc::execve(prepared.executable.as_ptr(), argv.as_ptr(), environment.as_ptr());
            child_error(4, errno());
        }
    }
    drop(gate_read);
    drop(out_write);
    drop(err_write);
    drop(setup_write);
    drop(null);
    let descriptor = match pidfd(pid) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            drop(gate_write);
            return Err(error);
        }
    };
    let until = Instant::now() + Duration::from_secs(5);
    let mut ready = [0_u8];
    loop {
        let mut poll = libc::pollfd {
            fd: setup_read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll, 1, 10) };
        if result > 0 {
            let result = unsafe { libc::read(setup_read.as_raw_fd(), ready.as_mut_ptr().cast(), 1) };
            if result == 1 && ready[0] == 0 {
                break;
            }
            let _ = signal_pidfd(&descriptor, libc::SIGKILL);
            return Err(ErrorCode::SetupFailed);
        }
        if Instant::now() >= until || readable(&descriptor)? {
            let _ = signal_pidfd(&descriptor, libc::SIGKILL);
            return Err(ErrorCode::SetupFailed);
        }
    }
    let (identity, group, session) = identity(pid)?;
    if group != pid || session != pid {
        return Err(ErrorCode::IdentityUnknown);
    }
    for fd in [&out_read, &err_read, &setup_read] {
        nonblocking(fd.as_raw_fd())?;
    }
    Ok(Root {
        pid,
        pidfd: descriptor,
        identity,
        gate: Some(gate_write),
        setup: setup_read,
        stdout: Some(out_read),
        stderr: Some(err_read),
        status: None,
        reaped: false,
    })
}

#[cfg(test)]
mod tests;
