use std::ffi::c_int;

/// The first descriptor after stdin, stdout and stderr.
pub(super) const FIRST_AFTER_STDIO: c_int = 3;

/// The descriptor count CPython's `safe_get_max_fd` assumes when `sysconf(_SC_OPEN_MAX)` has no answer.
#[cfg(not(target_vendor = "apple"))]
pub(super) const PYTHON_OPEN_MAX_FALLBACK: c_int = 256;
