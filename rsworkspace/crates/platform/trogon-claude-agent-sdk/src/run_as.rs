//! Resolution of Python's `ClaudeAgentOptions.user` (passed to `open_process(user=...)`, which is
//! `Popen(user=str)`: the name is looked up with `pwd.getpwnam`, that is libc `getpwnam`, so every name
//! service switch source is consulted, and only the real and effective uid change), and the password database home
//! folder `os.homedir()` falls back to.

use std::{ffi::CString, io, mem::MaybeUninit, ptr};
#[cfg(unix)]
use std::{
    ffi::{CStr, OsString},
    os::unix::ffi::OsStringExt,
    path::PathBuf,
};

use crate::constants::{PASSWD_INITIAL_BUFFER, PASSWD_MAX_BUFFER};

/// `pwd.getpwnam(name).pw_uid`, with Python's `KeyError` text when the name is absent.
#[cfg(unix)]
pub(crate) fn lookup(name: &str) -> io::Result<u32> {
    let not_found = || io::Error::new(io::ErrorKind::NotFound, format!("getpwnam(): name not found: '{name}'"));
    let c_name = CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "embedded null character in the user name"))?;
    let entry = passwd_entry(
        PASSWD_INITIAL_BUFFER,
        |record, buffer, found| {
            // SAFETY: `c_name` is NUL terminated and `passwd_entry` keeps the other arguments valid for the call.
            unsafe { libc::getpwnam_r(c_name.as_ptr(), record, buffer.as_mut_ptr().cast(), buffer.len(), found) }
        },
        |record| record.pw_uid,
    );
    entry
        .and_then(|uid| uid.ok_or(libc::ENOENT))
        .map_err(|code| lookup_error(code, not_found))
}

/// The status codes Python reads as an absent name, which `getpwnam_r` may report instead of a null result.
#[cfg(unix)]
fn lookup_error(code: libc::c_int, not_found: impl FnOnce() -> io::Error) -> io::Error {
    match code {
        libc::ENOENT | libc::ESRCH | libc::EBADF | libc::EPERM => not_found(),
        code => io::Error::from_raw_os_error(code),
    }
}

/// The home folder the password database gives the effective user, where `os.homedir()` looks when `HOME` is unset
/// (libuv `uv_os_get_passwd`).
#[cfg(unix)]
pub(crate) fn home_folder() -> io::Result<PathBuf> {
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let entry = passwd_entry(
        PASSWD_INITIAL_BUFFER,
        |record, buffer, found| {
            // SAFETY: `passwd_entry` keeps every argument valid for the call.
            unsafe { libc::getpwuid_r(uid, record, buffer.as_mut_ptr().cast(), buffer.len(), found) }
        },
        // SAFETY: a non-null `pw_dir` is a NUL terminated string inside the buffer `passwd_entry` still holds.
        |record| (!record.pw_dir.is_null()).then(|| unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes().to_vec()),
    );
    entry
        .map_err(io::Error::from_raw_os_error)?
        .flatten()
        .map(|directory| PathBuf::from(OsString::from_vec(directory)))
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

/// Runs one reentrant password database `query`, doubling its buffer while it reports `ERANGE` up to the cap, and
/// hands the record it finds to `read` while the buffer its strings point into is still alive. `None` is a query that
/// found nothing; an error is the status code the query returned.
#[cfg(unix)]
fn passwd_entry<T>(
    initial: usize,
    mut query: impl FnMut(*mut libc::passwd, &mut [u8], *mut *mut libc::passwd) -> libc::c_int,
    read: impl FnOnce(&libc::passwd) -> T,
) -> Result<Option<T>, libc::c_int> {
    let mut buffer = vec![0_u8; initial];
    loop {
        let mut record = MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = ptr::null_mut();
        match query(record.as_mut_ptr(), &mut buffer, &raw mut found) {
            0 if found.is_null() => return Ok(None),
            // SAFETY: a zero status with a non-null result means `record` was fully initialized.
            0 => return Ok(Some(read(unsafe { record.assume_init_ref() }))),
            libc::ERANGE if buffer.len() < PASSWD_MAX_BUFFER => buffer.resize(buffer.len() * 2, 0),
            code => return Err(code),
        }
    }
}

/// What `Popen(user=uid)` runs in the child before exec: `setreuid(uid, uid)` and nothing else, so supplementary
/// groups stay as they are (`Command::uid` would also call `setgroups`).
#[cfg(unix)]
pub(crate) fn set_real_and_effective_uid(uid: u32) -> io::Result<()> {
    // SAFETY: `setreuid` only takes integers and is async-signal-safe, so it may run between fork and exec.
    if unsafe { libc::setreuid(uid, uid) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
