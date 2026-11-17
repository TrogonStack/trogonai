//! Which open descriptors above stderr the CLI child keeps across `exec`.
//!
//! Python starts the CLI through `anyio.open_process`, which is `subprocess.Popen` with its default
//! `close_fds=True`: the child keeps only its stdio, on every platform. TypeScript starts it through Node's
//! `child_process.spawn`, that is libuv: on Apple platforms libuv calls `posix_spawn` with
//! `POSIX_SPAWN_CLOEXEC_DEFAULT`, so only the stdio passes, while elsewhere it forks and keeps every descriptor
//! that is not close-on-exec.
//!
//! Without this, a Rust parent leaks descriptors on Apple platforms even when they were opened close-on-exec: the
//! standard library there creates a pipe and only then marks it close-on-exec, so a spawn on another thread in
//! between hands the child the pipe's ends, and the parent never sees EOF on a pipe the child holds open.
//!
//! The child marks the descriptors close-on-exec rather than closing them, so the standard library's own pipe that
//! reports an `exec` failure keeps working.

use std::{ffi::c_int, io};

use crate::ArgvStyle;

mod constants;

use constants::FIRST_AFTER_STDIO;
#[cfg(not(target_vendor = "apple"))]
use constants::PYTHON_OPEN_MAX_FALLBACK;

/// Whether the child keeps only its stdio: always under the Python dialect, and on Apple platforms under the
/// TypeScript one.
pub(crate) fn passes_only_stdio(style: ArgvStyle) -> bool {
    style == ArgvStyle::Python || cfg!(target_vendor = "apple")
}

/// One past the highest descriptor the child looks at, read in the parent because `sysconf` is not
/// async-signal-safe.
#[cfg(not(target_vendor = "apple"))]
pub(crate) fn descriptor_limit() -> c_int {
    // SAFETY: `sysconf` only reads a configuration value.
    let open_max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    c_int::try_from(open_max)
        .ok()
        .filter(|max| *max >= 0)
        .unwrap_or(PYTHON_OPEN_MAX_FALLBACK)
}

/// One past the highest descriptor the process can open. Apple's `sysconf(_SC_OPEN_MAX)` answers the soft limit even
/// past the kernel's per-process cap, which `getdtablesize` takes into account.
#[cfg(target_vendor = "apple")]
pub(crate) fn descriptor_limit() -> c_int {
    // SAFETY: `getdtablesize` only reads the process descriptor limit.
    unsafe { libc::getdtablesize() }
}

/// Marks every descriptor above stderr close-on-exec. It runs in the child between `fork` and `exec`, so it only
/// makes async-signal-safe calls and allocates nothing.
pub(crate) fn keep_only_stdio(limit: c_int) -> io::Result<()> {
    let first = if all_marked() { limit } else { FIRST_AFTER_STDIO };
    mark_close_on_exec(first, limit);
    Ok(())
}

/// Marks every descriptor above stderr close-on-exec in one `close_range` call, when the kernel supports it.
#[cfg(target_os = "linux")]
fn all_marked() -> bool {
    // SAFETY: `close_range` with `CLOSE_RANGE_CLOEXEC` only changes descriptor flags.
    unsafe {
        libc::syscall(
            libc::SYS_close_range,
            FIRST_AFTER_STDIO.unsigned_abs(),
            std::ffi::c_uint::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        ) == 0
    }
}

#[cfg(not(target_os = "linux"))]
fn all_marked() -> bool {
    false
}

/// Marks the descriptors from `first` up to `limit` close-on-exec, skipping those that are not open.
pub(crate) fn mark_close_on_exec(first: c_int, limit: c_int) {
    for descriptor in first..limit {
        // SAFETY: `F_SETFD` only changes descriptor flags and fails harmlessly on a descriptor that is not open.
        unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
}

#[cfg(test)]
mod tests;
