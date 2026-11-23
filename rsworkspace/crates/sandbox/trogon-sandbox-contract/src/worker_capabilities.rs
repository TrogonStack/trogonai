use std::ops::{BitOr, BitOrAssign};

use serde::{Deserialize, Serialize};

use crate::{ProviderCapabilityProfile, WorkerCapabilitiesError};

/// Compact capability summary a worker advertises, serialized as its `u32` bits.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(into = "u32", try_from = "u32")]
pub struct WorkerCapabilities(u32);

impl WorkerCapabilities {
    pub const SUPPORTS_SNAPSHOT: Self = Self(1);
    pub const SUPPORTS_S3_VOLUMES: Self = Self(1 << 1);
    pub const SUPPORTS_RESTORE: Self = Self(1 << 2);
    pub const SUPPORTS_FUSE_MOUNTS: Self = Self(1 << 3);
    pub const SUPPORTS_PRIVILEGED_EXEC: Self = Self(1 << 4);
    pub const SUPPORTS_SANDBOX_USERS: Self = Self(1 << 5);
    pub const SUPPORTS_INTERACTIVE_STDIN: Self = Self(1 << 6);
    pub const SUPPORTS_NAMED_PROCESSES: Self = Self(1 << 7);
    pub const SUPPORTS_PROCESS_READINESS: Self = Self(1 << 8);

    const KNOWN_BITS: u32 = (1 << 9) - 1;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn all() -> Self {
        Self(Self::KNOWN_BITS)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for WorkerCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for WorkerCapabilities {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl TryFrom<u32> for WorkerCapabilities {
    type Error = WorkerCapabilitiesError;

    fn try_from(bits: u32) -> Result<Self, Self::Error> {
        if bits & !Self::KNOWN_BITS == 0 {
            Ok(Self(bits))
        } else {
            Err(WorkerCapabilitiesError::UnknownBits(bits))
        }
    }
}

impl From<WorkerCapabilities> for u32 {
    fn from(capabilities: WorkerCapabilities) -> Self {
        capabilities.0
    }
}

impl From<&ProviderCapabilityProfile> for WorkerCapabilities {
    fn from(profile: &ProviderCapabilityProfile) -> Self {
        let isolation = &profile.isolation;
        [
            (profile.snapshot.can_snapshot, Self::SUPPORTS_SNAPSHOT),
            (profile.volume.supports_s3_backend, Self::SUPPORTS_S3_VOLUMES),
            (profile.snapshot.can_restore, Self::SUPPORTS_RESTORE),
            (profile.volume.supports_fuse_mounts, Self::SUPPORTS_FUSE_MOUNTS),
            (isolation.supports_privileged, Self::SUPPORTS_PRIVILEGED_EXEC),
            (isolation.supports_sandbox_users, Self::SUPPORTS_SANDBOX_USERS),
            (isolation.supports_interactive_stdin, Self::SUPPORTS_INTERACTIVE_STDIN),
            (isolation.supports_named_processes, Self::SUPPORTS_NAMED_PROCESSES),
            (isolation.supports_process_readiness, Self::SUPPORTS_PROCESS_READINESS),
        ]
        .into_iter()
        .filter(|(supported, _)| *supported)
        .fold(Self::empty(), |bits, (_, flag)| bits | flag)
    }
}
