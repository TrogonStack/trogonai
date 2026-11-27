use crate::constants::MAX_LEASES_LIMIT;

/// A worker's settings contradict each other or the platform's limits.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum FlyConfigError {
    #[error("max leases must be from 1 to {MAX_LEASES_LIMIT}, not {0}")]
    MaxLeases(usize),
    #[error("the boot timeout must be non-zero")]
    BootTimeout,
    #[error("warm pool exceeds worker capacity")]
    WarmPoolExceedsCapacity,
    #[error("a configured persistent volume requires one lease and no warm pool")]
    VolumeNeedsSingleLease,
    #[error("a workspace volume and an operator volume binding cannot be combined")]
    WorkspaceVolumeWithOperatorVolume,
    #[error("a workspace volume needs a fresh Machine per lease, so it cannot be combined with a warm pool")]
    WorkspaceVolumeWithWarmPool,
}
