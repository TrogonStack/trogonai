use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, RawFd};

use crate::args::ControlFd;

pub(super) fn open_reader(fd: ControlFd) -> Result<File, String> {
    let raw = fd.raw();
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if flags & libc::O_ACCMODE == libc::O_WRONLY {
        return Err("it is open for writing only".to_string());
    }
    let private = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, raw + 1) };
    if private < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let reader = unsafe { File::from_raw_fd(private) };
    displace_with_null(raw).map_err(|err| format!("cannot keep it from the command: {err}"))?;
    Ok(reader)
}

fn displace_with_null(fd: RawFd) -> io::Result<()> {
    let null = File::open("/dev/null")?;
    let status = unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&null), fd) };
    if status < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
