pub const OBSERVE_FLAG: &str = "--observe";

pub(crate) const GUEST_OBSERVER_BINARY: &str = "/dev/sandbox-observe/observer";

pub(super) const GUEST_EVENT_SOCKET: &str = "/dev/sandbox-observe/events.sock";

pub(super) const MAX_EVENT_BYTES: usize = 8192;

pub(super) const SECCOMP_SET_MODE_FILTER: libc::c_long = 1;

pub(super) const SECCOMP_FILTER_FLAG_NEW_LISTENER: libc::c_long = 1 << 3;

pub(super) const SECCOMP_USER_NOTIF_FLAG_CONTINUE: u32 = 1;

pub(super) const POLL_INTERVAL_MS: libc::c_int = 100;

pub(super) const PAGE: u64 = 4096;

#[cfg(target_arch = "x86_64")]
pub(super) const AUDIT_ARCH: u32 = 0xc000_003e;

#[cfg(target_arch = "aarch64")]
pub(super) const AUDIT_ARCH: u32 = 0xc000_00b7;

pub(super) const RET_USER_NOTIF: u32 = 0x7fc0_0000;

pub(super) const OFFSET_NR: u32 = 0;

pub(super) const OFFSET_ARCH: u32 = 4;

pub(super) const OFFSET_ARGS: u32 = 16;

pub(super) const SYS_OPENAT2: i64 = 437;

pub(super) const SYS_FCHMODAT2: i64 = 452;

pub(super) const WRITE_INTENT: u32 =
    (libc::O_WRONLY | libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC | libc::O_APPEND) as u32;

pub(super) const AT_FDCWD: i32 = -100;

pub(super) const PATH_MAX: usize = 4096;
