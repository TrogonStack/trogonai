//! The passwd database lookups behind `os.homedir()` (the effective user, libuv `uv_os_get_passwd`) and Python's
//! `pwd.getpwuid(os.getuid())` (the real user).

use std::ffi::OsString;

#[cfg(unix)]
use crate::constants::{PASSWD_INITIAL_BUFFER, PASSWD_MAX_BUFFER};

/// The `pw_dir` of one user id as `getpwuid_r` reports it; `None` when the database has no such user.
#[cfg(unix)]
pub(crate) fn home_of_uid(uid: u32) -> Option<OsString> {
    home_of_uid_with_buffer(uid, PASSWD_INITIAL_BUFFER, PASSWD_MAX_BUFFER)
}

#[cfg(unix)]
fn home_of_uid_with_buffer(uid: u32, initial: usize, maximum: usize) -> Option<OsString> {
    use std::{ffi::CStr, mem::MaybeUninit, os::unix::ffi::OsStringExt, ptr};

    let mut buffer = vec![0_u8; initial];
    loop {
        let mut record = MaybeUninit::<libc::passwd>::uninit();
        let mut found: *mut libc::passwd = ptr::null_mut();
        // SAFETY: `record` and `found` are valid for writes, and `buffer` is a live allocation whose length is
        // passed alongside its pointer.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &raw mut found,
            )
        };
        match status {
            0 if found.is_null() => return None,
            0 => {
                // SAFETY: a zero status with a non-null result means `record` was fully initialized and `pw_dir`
                // points to a NUL terminated string inside `buffer`, which is still alive.
                let directory = unsafe { CStr::from_ptr(record.assume_init().pw_dir) };
                return Some(OsString::from_vec(directory.to_bytes().to_vec()));
            }
            libc::ERANGE if buffer.len() < maximum => buffer.resize(buffer.len() * 2, 0),
            _ => return None,
        }
    }
}

/// The home of the effective user, which is what libuv reads when `HOME` is unset.
#[cfg(unix)]
pub(crate) fn effective_user_home() -> Option<OsString> {
    // SAFETY: `geteuid` takes no arguments and always succeeds.
    home_of_uid(unsafe { libc::geteuid() })
}

/// The home of the real user, which is what Python's `os.path.expanduser` reads when `HOME` is unset.
#[cfg(unix)]
pub(crate) fn real_user_home() -> Option<OsString> {
    // SAFETY: `getuid` takes no arguments and always succeeds.
    home_of_uid(unsafe { libc::getuid() })
}

/// The profile directory of the process token's user, which is what libuv's `uv_os_get_passwd` reads on Windows
/// when `USERPROFILE` is unset: `OpenProcessToken` then `GetUserProfileDirectoryW` with a sized buffer.
#[cfg(windows)]
pub(crate) fn effective_user_home() -> Option<OsString> {
    use std::{os::windows::ffi::OsStringExt, ptr};

    const TOKEN_QUERY: u32 = 0x0008;
    type Handle = *mut core::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    }
    #[link(name = "userenv")]
    unsafe extern "system" {
        fn GetUserProfileDirectoryW(token: Handle, directory: *mut u16, size: *mut u32) -> i32;
    }

    let mut token: Handle = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo handle that needs no closing, and `token` is valid for writes.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return None;
    }
    let mut size: u32 = 0;
    // SAFETY: `token` is the open process token; a null buffer with a zero size only asks for the needed length.
    unsafe { GetUserProfileDirectoryW(token, ptr::null_mut(), &raw mut size) };
    let mut buffer = vec![0_u16; size as usize];
    // SAFETY: `buffer` holds `size` UTF-16 units, which is the length passed alongside its pointer.
    let found = size != 0 && unsafe { GetUserProfileDirectoryW(token, buffer.as_mut_ptr(), &raw mut size) } != 0;
    // SAFETY: `token` was opened above and is closed exactly once.
    unsafe { CloseHandle(token) };
    if !found {
        return None;
    }
    let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    Some(OsString::from_wide(&buffer[..length]))
}

/// Targets with neither a passwd database nor a Windows token have no account home to read.
#[cfg(not(any(unix, windows)))]
pub(crate) fn effective_user_home() -> Option<OsString> {
    None
}

/// Python never consults a passwd database on Windows, so there is no real-user entry to read there.
#[cfg(not(unix))]
pub(crate) fn real_user_home() -> Option<OsString> {
    None
}

#[cfg(all(test, unix))]
mod tests;
