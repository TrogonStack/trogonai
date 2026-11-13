//! Resolution of Python's `ClaudeAgentOptions.user` (passed to `open_process(user=...)`, which is
//! `Popen(user=str)`: the name is looked up with `pwd.getpwnam`, that is libc `getpwnam`, so every name
//! service switch source is consulted, and only the real and effective uid change).

use std::{ffi::CString, io, mem::MaybeUninit, ptr};

use crate::constants::{PASSWD_INITIAL_BUFFER, PASSWD_MAX_BUFFER};

/// `pwd.getpwnam(name).pw_uid`, with Python's `KeyError` text when the name is absent.
#[cfg(unix)]
pub(crate) fn lookup(name: &str) -> io::Result<u32> {
    let not_found = || io::Error::new(io::ErrorKind::NotFound, format!("getpwnam(): name not found: '{name}'"));
    let c_name = CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "embedded null character in the user name"))?;
    let mut buffer = vec![0_u8; PASSWD_INITIAL_BUFFER];
    loop {
        let mut record = MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = ptr::null_mut();
        // SAFETY: `c_name` is NUL terminated, `record` and `found` are valid for writes, and `buffer` is a live
        // allocation whose length is passed alongside its pointer.
        let status = unsafe {
            libc::getpwnam_r(
                c_name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &raw mut found,
            )
        };
        match status {
            0 if found.is_null() => return Err(not_found()),
            // SAFETY: a zero status with a non-null result means `record` was fully initialized.
            0 => return Ok(unsafe { record.assume_init() }.pw_uid),
            libc::ERANGE if buffer.len() < PASSWD_MAX_BUFFER => buffer.resize(buffer.len() * 2, 0),
            libc::ENOENT | libc::ESRCH | libc::EBADF | libc::EPERM => return Err(not_found()),
            code => return Err(io::Error::from_raw_os_error(code)),
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
