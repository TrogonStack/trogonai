//! Checks that an open descriptor still lies inside the sandbox root.

#[cfg(target_os = "macos")]
use std::ffi::{CStr, OsStr};
#[cfg(target_os = "linux")]
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Stands in for the descriptor trait where there are no descriptors to ask about.
#[cfg(not(unix))]
pub(crate) trait AsRawFd {}
#[cfg(not(unix))]
impl<T> AsRawFd for T {}

pub(crate) fn require_inside(file: &impl AsRawFd, root: &Path) -> io::Result<()> {
    if opened_inside(file, root)? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path resolves outside the sandbox root",
        ))
    }
}

/// Whether the file behind an open descriptor lies inside `root`, judged from the path the kernel
/// reports for the descriptor rather than the path that was opened.
#[cfg(target_os = "linux")]
pub(crate) fn opened_inside(file: &impl AsRawFd, root: &Path) -> io::Result<bool> {
    let real = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    Ok(real.starts_with(root))
}

#[cfg(target_os = "macos")]
pub(crate) fn opened_inside(file: &impl AsRawFd, root: &Path) -> io::Result<bool> {
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes a NUL-terminated path of at most PATH_MAX bytes into the
    // buffer, which is PATH_MAX bytes long and outlives the call.
    let status = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) };
    if status == -1 {
        return Err(io::Error::last_os_error());
    }
    let real = CStr::from_bytes_until_nul(&buffer).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Path::new(OsStr::from_bytes(real.to_bytes())).starts_with(root))
}

/// Elsewhere there is no portable way to ask where a descriptor points, so the check made before
/// the open is all there is.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn opened_inside(_file: &impl AsRawFd, _root: &Path) -> io::Result<bool> {
    Ok(true)
}
