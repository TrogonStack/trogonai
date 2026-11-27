#[cfg(target_os = "linux")]
use seccompiler::TargetArch;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, AtomicI32};
#[cfg(target_os = "macos")]
use std::sync::{Mutex, Once};
#[cfg(unix)]
use std::time::Duration;

#[cfg(target_os = "linux")]
pub(super) const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[cfg(target_os = "linux")]
pub(super) const ROOT_SYSTEM_DIRS: &[&str] = &["/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32"];

#[cfg(target_os = "linux")]
pub(super) const FD_HEADROOM: u64 = 64;

#[cfg(target_os = "linux")]
pub const INIT_FLAG: &str = "--guest-init";

#[cfg(target_os = "linux")]
pub const INIT_EXIT_CODE: i32 = 71;

#[cfg(target_os = "linux")]
pub(super) const GUEST_INIT_BINARY: &str = "/dev/sandbox-init/init";

#[cfg(unix)]
pub(super) const CGROUP_MOUNT: &str = "/sys/fs/cgroup";

#[cfg(unix)]
pub(super) const CPU_PERIOD_MICROS: u32 = 100_000;

#[cfg(unix)]
pub(super) const REMOVE_RETRIES: u32 = 100;

#[cfg(unix)]
pub(super) const REMOVE_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(unix)]
/// Files protected wherever they appear, compared ignoring ASCII case.
pub const DANGEROUS_FILES: [&str; 9] = [
    ".gitconfig",
    ".gitmodules",
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".ripgreprc",
    ".mcp.json",
];

#[cfg(unix)]
/// Directories protected whole wherever they appear.
pub const DANGEROUS_DIRECTORIES: [&str; 2] = [".vscode", ".idea"];

#[cfg(unix)]
pub const DOT_AGENT: &str = ".agent";

#[cfg(unix)]
pub const DOT_AGENT_PROTECTED: [&str; 2] = ["commands", "agents"];

#[cfg(unix)]
pub const DOT_GIT: &str = ".git";

#[cfg(unix)]
#[cfg(target_os = "macos")]
pub const GIT_HOOKS: &str = "hooks";

#[cfg(unix)]
/// Entries of a git directory that git reads and may execute.
pub const GIT_DIR_PROTECTED: [&str; 3] = ["hooks", "config", "config.worktree"];

#[cfg(unix)]
/// What stays protected when the lease leaves `config` writable: the hooks
/// and the per-worktree config, which can name executables the same way.
pub const GIT_DIR_PROTECTED_WITHOUT_CONFIG: [&str; 2] = ["hooks", "config.worktree"];

#[cfg(target_os = "linux")]
/// Paths protected at the top of every writable root even when they do not
/// exist yet. `.git` is absent on purpose: guarding a missing `.git` would
/// stop `git init`.
pub const ROOT_PATHS: [&str; 13] = [
    ".gitconfig",
    ".gitmodules",
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".ripgreprc",
    ".mcp.json",
    ".vscode",
    ".idea",
    ".agent/commands",
    ".agent/agents",
];

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub const DEFAULT_WRITE_DEVICES: [&str; 6] = [
    "/dev/stdout",
    "/dev/stderr",
    "/dev/null",
    "/dev/tty",
    "/dev/dtracehelper",
    "/dev/autofs_nowait",
];

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// The directories the sandbox hands out as `TMPDIR` when nothing overrides
/// it, writable like the devices.
pub const DEFAULT_WRITE_DIRS: [&str; 2] = ["/tmp/sandbox", "/private/tmp/sandbox"];

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub const RUNTIME_MARKER_VAR: &str = "TROGON_SANDBOXED";

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub const RUNTIME_MARKER_VALUE: &str = "1";

#[cfg(target_os = "linux")]
#[cfg(target_arch = "x86_64")]
pub(super) const TARGET_ARCH: TargetArch = TargetArch::x86_64;

#[cfg(target_os = "linux")]
#[cfg(target_arch = "aarch64")]
pub(super) const TARGET_ARCH: TargetArch = TargetArch::aarch64;

#[cfg(target_os = "linux")]
/// Numbers from the unified syscall table that x86_64 and aarch64 share; the
/// libc crate does not name `open_tree_attr` yet.
pub(super) const SYS_MOUNT_SETATTR: i64 = 442;

#[cfg(target_os = "linux")]
pub(super) const SYS_OPEN_TREE_ATTR: i64 = 467;

#[cfg(target_os = "linux")]
pub(super) const NAMESPACE_SYSCALLS: &[i64] = &[
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_setns,
    libc::SYS_open_tree,
    libc::SYS_move_mount,
    libc::SYS_fsopen,
    libc::SYS_fsconfig,
    libc::SYS_fsmount,
    libc::SYS_fspick,
    SYS_MOUNT_SETATTR,
    SYS_OPEN_TREE_ATTR,
];

#[cfg(target_os = "linux")]
pub(super) const IO_URING_SYSCALLS: &[i64] = &[
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
];

#[cfg(target_os = "linux")]
pub(super) const DENIED_SOCKET_DOMAINS: &[libc::c_int] = &[libc::AF_UNIX, libc::AF_VSOCK];

#[cfg(target_os = "linux")]
/// `unshare` alone takes this bit, from Linux 7.1, for a mount namespace
/// holding only the root mount. To `clone` it is `CLONE_PARENT_SETTID`,
/// which threads are made with.
pub(super) const UNSHARE_EMPTY_MNTNS: libc::c_int = 0x0010_0000;

#[cfg(target_os = "linux")]
/// Ioctl requests that push input into a terminal or reprogram the console.
/// `--new-session` keeps a payload from reaching the worker's terminal, but a
/// terminal launch must keep its own session, so these stay refused for every
/// payload regardless.
pub(super) const DENIED_TTY_IOCTLS: &[libc::c_ulong] = &[libc::TIOCSTI, libc::TIOCLINUX];

#[cfg(target_os = "linux")]
pub(super) const DENIED_UNSHARE_FLAGS: &[libc::c_int] = &[libc::CLONE_NEWUSER, libc::CLONE_NEWNS, UNSHARE_EMPTY_MNTNS];

#[cfg(target_os = "linux")]
pub(super) const DENIED_CLONE_FLAGS: &[libc::c_int] = &[libc::CLONE_NEWUSER, libc::CLONE_NEWNS];

#[cfg(target_os = "linux")]
pub(super) const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;

#[cfg(target_os = "linux")]
pub(super) const X32_SYSCALL_BIT: u32 = 0x4000_0000;

#[cfg(target_os = "linux")]
pub(super) const SECCOMP_DATA_NR_OFFSET: u32 = 0;

#[cfg(target_os = "linux")]
pub(super) const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;

#[cfg(target_os = "linux")]
pub(super) const DISABLE_USERNS_FLAG: &str = "--disable-userns";

#[cfg(target_os = "linux")]
pub(super) const SETUID_BIT: u32 = 0o4000;

#[cfg(target_os = "linux")]
pub(super) const CAP_SETFCAP_BIT: u32 = 31;

#[cfg(target_os = "linux")]
pub(super) static NO_USERNS_LIMIT_LOGGED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "linux")]
pub(super) const NO_USERNS_LIMIT_WARNING: &str = "[Sandbox Linux] no seccomp helper in use, and bubblewrap cannot disable user namespaces (needs 0.8.0 or later, not setuid, and no enableWeakerNestedSandbox) - a sandboxed command that creates one can undo the write denies";

#[cfg(target_os = "linux")]
pub(super) const RESOLVE_NO_MAGICLINKS: u64 = 0x02;

#[cfg(target_os = "linux")]
pub(super) const RESOLVE_NO_SYMLINKS: u64 = 0x04;

#[cfg(target_os = "linux")]
pub const MAX_SYMLINK_HOPS: usize = 40;

#[cfg(target_os = "linux")]
pub const MAX_GLOB_VISITS: usize = 100_000;

#[cfg(target_os = "linux")]
pub const MAX_MANDATORY_VISITS: usize = MAX_GLOB_VISITS;

#[cfg(target_os = "linux")]
pub const MAX_BWRAP_ARGUMENTS: usize = 9000;

#[cfg(target_os = "linux")]
pub const RESERVED_BWRAP_ARGUMENTS: usize = 100;

#[cfg(target_os = "linux")]
pub(super) const MAX_GIT_POINTER_BYTES: usize = 4096;

#[cfg(target_os = "linux")]
pub(super) const PRIVATE_REGIONS: [&str; 3] = ["/proc", "/dev", "/tmp"];

#[cfg(target_os = "linux")]
pub(super) const HOST_VIEW_PRIVATE_REGIONS: [&str; 2] = ["/proc", "/dev"];

#[cfg(target_os = "linux")]
pub(super) const BUSY_RETRIES: u32 = 20;

#[cfg(target_os = "linux")]
pub(super) const BUSY_BACKOFF: Duration = Duration::from_millis(10);

pub const NAME_PREFIX: &str = "trg-sbx-";

pub(super) const DIRECTORY: &str = ".appcontainer-profiles";

#[cfg(target_os = "linux")]
pub const NESTED_FLAG: &str = "--nested";

#[cfg(target_os = "linux")]
pub const NESTED_EXIT_CODE: i32 = 72;

#[cfg(target_os = "linux")]
pub(super) const NEW_PID_AND_MOUNT: libc::c_int = libc::CLONE_NEWPID | libc::CLONE_NEWNS;

#[cfg(target_os = "linux")]
pub(super) const FORWARDED_SIGNALS: [libc::c_int; 6] = [
    libc::SIGTERM,
    libc::SIGINT,
    libc::SIGHUP,
    libc::SIGQUIT,
    libc::SIGUSR1,
    libc::SIGUSR2,
];

#[cfg(target_os = "linux")]
pub(super) static FORWARD_TARGET: AtomicI32 = AtomicI32::new(-1);

#[cfg(target_os = "linux")]
pub const CONFINE_FLAG: &str = "--confine";

#[cfg(target_os = "linux")]
pub const CONFINE_EXIT_CODE: i32 = 71;

#[cfg(target_os = "linux")]
pub(super) const GUEST_LAUNCHER_BINARY: &str = "/dev/sandbox-confine/launcher";

#[cfg(target_os = "linux")]
pub(super) const DEV: &str = "/dev";

#[cfg(target_os = "linux")]
pub(super) const DEV_SHM: &str = "/dev/shm";

#[cfg(target_os = "linux")]
pub(super) const PROC: &str = "/proc";

#[cfg(target_os = "linux")]
pub(super) const TMP: &str = "/tmp";

#[cfg(target_os = "macos")]
pub(super) static HELD: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

#[cfg(target_os = "macos")]
pub(super) static HOOK: Once = Once::new();

#[cfg(target_os = "linux")]
pub const RELAY_FLAG: &str = "--relay";

#[cfg(target_os = "linux")]
pub const RELAY_PORT: u16 = 3128;

#[cfg(target_os = "linux")]
pub const RELAY_SOCKS_PORT: u16 = 1080;

#[cfg(target_os = "linux")]
pub const RELAY_EXIT_CODE: i32 = 70;

#[cfg(target_os = "linux")]
pub(super) const GUEST_RELAY_BINARY: &str = "/dev/sandbox-egress/relay";

#[cfg(target_os = "linux")]
pub(super) const GUEST_PROXY_SOCKET: &str = "/dev/sandbox-egress/proxy.sock";

#[cfg(target_os = "linux")]
pub(super) const RELAY_PROCESS_NAME: &std::ffi::CStr = c"egress-relay";

#[cfg(target_os = "linux")]
pub(super) const ACCEPT_RETRY: Duration = Duration::from_millis(50);

pub(super) const GENERATED_SUFFIX_LEN: usize = 32;
