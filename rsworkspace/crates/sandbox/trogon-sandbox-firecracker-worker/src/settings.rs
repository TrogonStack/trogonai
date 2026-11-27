//! The small value types operator configuration is built from. Every module that needs one
//! takes it from here, so none has to depend on the configuration as a whole.

use std::fmt;
use std::num::{NonZeroU8, NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

mod constants;
pub use constants::DEFAULT_AGENT_VSOCK_PORT;
use constants::MIN_GUEST_CID;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("guest CID must be 3 or greater, got {0}")]
    GuestCid(u32),
    #[error("{what} must be greater than zero")]
    Zero { what: &'static str },
    #[error("invalid {what}: {value:?}")]
    Invalid { what: &'static str, value: String },
}

macro_rules! path_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Eq, PartialEq)]
        pub struct $name(PathBuf);

        impl $name {
            pub fn new(path: impl Into<PathBuf>) -> Self {
                Self(path.into())
            }

            pub fn as_path(&self) -> &Path {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(PathBuf::from(s)))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.display().fmt(f)
            }
        }
    };
}

path_newtype!(
    /// The `firecracker` executable.
    FirecrackerBinary
);
path_newtype!(
    /// Uncompressed guest kernel image (`vmlinux`).
    KernelImage
);
path_newtype!(
    /// ext4 image with the guest agent preinstalled; copied once per lease.
    BaseRootfs
);
path_newtype!(
    /// Directory holding one subdirectory per lease.
    StateDir
);
path_newtype!(
    /// Where checkpoints of running leases are kept. Must not be inside the state dir, whose
    /// every subdirectory is taken for a lease left by a previous run.
    SnapshotDir
);
path_newtype!(
    /// The root helper (`scripts/fc-netns`) that creates and deletes lease network
    /// namespaces. Run through `sudo -n`, never as the worker's own user.
    NetnsHelper
);
path_newtype!(
    /// `sandbox-egress-proxyd`, started once per lease with a network device.
    EgressProxyBinary
);
path_newtype!(
    /// The worker's own executable, which doubles as the relay inside a lease namespace.
    RelayBinary
);

/// Starts every lease namespace name, so cleanup only ever touches this worker's.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NetnsPrefix(String);

impl NetnsPrefix {
    pub const MAX_LEN: usize = 16;

    pub fn new(value: impl Into<String>) -> Result<Self, ConfigError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_LEN
            && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(ConfigError::Invalid {
                what: "network namespace prefix",
                value,
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a worker needs to give leases a network device. Without it, every guest boots
/// with no NIC at all.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub helper: NetnsHelper,
    pub proxy_binary: EgressProxyBinary,
    pub relay_binary: RelayBinary,
    pub prefix: NetnsPrefix,
}

/// What a worker needs to checkpoint running leases and restore new ones from them.
#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    pub dir: SnapshotDir,
    /// Each snapshot holds the guest's whole memory, so only the newest are kept.
    pub keep: NonZeroUsize,
}

impl SnapshotConfig {
    pub const DEFAULT_KEEP: NonZeroUsize = match NonZeroUsize::new(4) {
        Some(keep) => keep,
        None => NonZeroUsize::MIN,
    };

    pub fn new(dir: SnapshotDir) -> Self {
        Self {
            dir,
            keep: Self::DEFAULT_KEEP,
        }
    }
}

/// The context ID a guest answers to on the vsock.
///
/// Every VM can use the same CID: host-initiated connections go through a per-VM Unix
/// socket, so CIDs never have to be unique across VMs on one host.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct GuestCid(u32);

impl GuestCid {
    pub const DEFAULT: Self = Self(MIN_GUEST_CID);

    pub fn new(value: u32) -> Result<Self, ConfigError> {
        if value < MIN_GUEST_CID {
            return Err(ConfigError::GuestCid(value));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

/// A port the guest agent listens on over vsock.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct VsockPort(u32);

impl VsockPort {
    pub const DEFAULT_AGENT: Self = Self(DEFAULT_AGENT_VSOCK_PORT);

    pub fn new(value: u32) -> Result<Self, ConfigError> {
        if value == 0 {
            return Err(ConfigError::Zero { what: "vsock port" });
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for VsockPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct VcpuCount(NonZeroU8);

impl VcpuCount {
    pub const DEFAULT: Self = Self(NonZeroU8::MIN.saturating_add(1));

    pub fn new(value: u8) -> Result<Self, ConfigError> {
        NonZeroU8::new(value)
            .map(Self)
            .ok_or(ConfigError::Zero { what: "vcpu count" })
    }

    pub fn get(self) -> u8 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct MemoryMib(NonZeroU32);

impl MemoryMib {
    pub const DEFAULT: Self = Self(match NonZeroU32::new(512) {
        Some(memory) => memory,
        None => NonZeroU32::MIN,
    });

    pub fn new(value: u32) -> Result<Self, ConfigError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(ConfigError::Zero { what: "memory" })
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone)]
pub struct RetentionConfig {
    pub max_bytes: u64,
    pub max_records: usize,
    pub export_ttl: Duration,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            max_bytes: 4 * 1024 * 1024 * 1024,
            max_records: 256,
            export_ttl: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WarmPoolConfig {
    pub max_snapshots: usize,
    pub max_bytes: u64,
    pub max_idle: Duration,
}

impl Default for WarmPoolConfig {
    fn default() -> Self {
        Self {
            max_snapshots: 0,
            max_bytes: 4 * 1024 * 1024 * 1024,
            max_idle: Duration::from_secs(300),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OciRootfsConfig {
    pub cache_dir: PathBuf,
    pub guest_agent: PathBuf,
    pub guest_init: PathBuf,
    pub umoci: PathBuf,
    pub mkfs_ext4: PathBuf,
    pub disk_size_bytes: u64,
    pub max_cache_bytes: u64,
    pub max_download_bytes: u64,
}
