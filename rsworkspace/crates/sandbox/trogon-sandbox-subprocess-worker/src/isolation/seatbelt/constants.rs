use std::time::Duration;

pub(crate) const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub(crate) const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

pub(crate) const TAG_PREFIX: &str = "TRGSBX_";

pub(crate) const TAG_RANDOM_BYTES: usize = 8;

pub(crate) const LIVENESS_POLL: Duration = Duration::from_millis(250);

pub(crate) const MONITOR_GRACE: Duration = Duration::from_secs(1);

pub(crate) const INLINE_PROFILE_BYTES: usize = 128 * 1024;

pub(crate) const LOG_BINARY: &str = "/usr/bin/log";

pub(crate) const SANDBOX_MARKER: &str = "Sandbox: ";

pub(crate) const NOISE: [&str; 3] = ["com.apple.diagnosticd", "com.apple.analyticsd", "mDNSResponder"];

pub(crate) const MACH_SERVICES: &[&str] = &[
    "com.apple.audio.systemsoundserver",
    "com.apple.distributed_notifications@Uv3",
    "com.apple.FontObjectsServer",
    "com.apple.fonts",
    "com.apple.logd",
    "com.apple.lsd.mapdb",
    "com.apple.PowerManagement.control",
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.bsd.dirhelper",
    "com.apple.securityd.xpc",
    "com.apple.coreservices.launchservicesd",
    "com.apple.sysmond",
];

pub(crate) const SYSCTL_NAMES: &[&str] = &[
    "hw.activecpu",
    "hw.busfrequency_compat",
    "hw.byteorder",
    "hw.cacheconfig",
    "hw.cachelinesize_compat",
    "hw.cpufamily",
    "hw.cpufrequency",
    "hw.cpufrequency_compat",
    "hw.cputype",
    "hw.l1dcachesize_compat",
    "hw.l1icachesize_compat",
    "hw.l2cachesize_compat",
    "hw.l3cachesize_compat",
    "hw.logicalcpu",
    "hw.logicalcpu_max",
    "hw.machine",
    "hw.memsize",
    "hw.ncpu",
    "hw.nperflevels",
    "hw.packages",
    "hw.pagesize_compat",
    "hw.pagesize",
    "hw.physicalcpu",
    "hw.physicalcpu_max",
    "hw.tbfrequency_compat",
    "hw.vectorunit",
    "kern.argmax",
    "kern.bootargs",
    "kern.hostname",
    "kern.maxfiles",
    "kern.maxfilesperproc",
    "kern.maxproc",
    "kern.ngroups",
    "kern.osproductversion",
    "kern.osrelease",
    "kern.ostype",
    "kern.osvariant_status",
    "kern.osversion",
    "kern.secure_kernel",
    "kern.tcsm_available",
    "kern.tcsm_enable",
    "kern.usrstack64",
    "kern.version",
    "kern.willshutdown",
    "machdep.cpu.brand_string",
    "machdep.ptrauth_enabled",
    "security.mac.lockdown_mode_state",
    "sysctl.proc_cputype",
    "vm.loadavg",
];

pub(crate) const SYSCTL_PREFIXES: &[&str] = &[
    "hw.optional.arm",
    "hw.optional.arm.",
    "hw.optional.armv8_",
    "hw.perflevel",
    "kern.proc.all",
    "kern.proc.pgrp.",
    "kern.proc.pid.",
    "machdep.cpu.",
    "net.routetable.",
];

pub(crate) const IOCTL_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/dtracehelper",
    "/dev/tty",
];

/// Literal denies that share a directory are merged into one regex once there
/// are this many of them.
pub(crate) const MIN_GROUP_SIZE: usize = 4;

/// The budget of one generated regex, below the limit of a profile string.
pub(crate) const MAX_REGEX_BYTES: usize = 900;

pub(crate) const SUBTREE_TAIL: &str = "(/.*)?$";

pub(crate) const BENEATH_TAIL: &str = "/.*$";

/// How many characters a glob set may expand to when it is not a plain
/// alphanumeric range.
pub(crate) const MAX_SET_SPAN: u32 = 64;

/// `sandbox-exec` refuses a string literal longer than 1024 bytes.
pub(crate) const MAX_STRING_BYTES: usize = 1000;

/// A profile past this size would take `sandbox-exec` minutes to compile.
pub(crate) const MAX_PROFILE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) const TRUST_SERVICE: &str = "com.apple.trustd.agent";

pub(crate) const APPLE_EVENTS_SERVICES: [&str; 3] = [
    "com.apple.coreservices.appleevents",
    "com.apple.CoreServices.coreservicesd",
    "com.apple.coreservices.quarantine-resolver",
];

pub(crate) const PTY_DEVICE: &str = "/dev/ptmx";

pub(crate) const PTY_SLAVES: &str = "^/dev/ttys";

pub(crate) const DEVICE_DIRECTORY: &str = "/dev";

pub(crate) const DEVICE_DIRECTORY_PREFIX: &str = "/dev/";
