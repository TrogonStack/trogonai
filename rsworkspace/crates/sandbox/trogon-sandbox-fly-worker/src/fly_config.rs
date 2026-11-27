use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::constants::MAX_LEASES_LIMIT;
use crate::{FlyConfigError, FlyConfigParts, FlyPreviewNetwork, FlyRegion, FlyVolumeBinding, FlyWorkspaceVolume};

/// A worker's settings, checked to be consistent with each other.
#[derive(Debug, Clone)]
pub struct FlyConfig {
    pub(crate) image: String,
    pub(crate) region: FlyRegion,
    pub(crate) state_dir: PathBuf,
    pub(crate) boot_timeout: Duration,
    pub(crate) max_leases: usize,
    pub(crate) memory_mib: NonZeroU32,
    pub(crate) cpus: NonZeroU32,
    pub(crate) warm_pool_size: usize,
    pub(crate) volume: Option<FlyVolumeBinding>,
    pub(crate) workspace_volume: Option<FlyWorkspaceVolume>,
    pub(crate) egress_proxy: Option<String>,
    pub(crate) public_host: Option<String>,
    pub(crate) preview_network: FlyPreviewNetwork,
}

impl FlyConfig {
    pub fn new(parts: FlyConfigParts) -> Result<Self, FlyConfigError> {
        if parts.max_leases == 0 || parts.max_leases > MAX_LEASES_LIMIT {
            return Err(FlyConfigError::MaxLeases(parts.max_leases));
        }
        if parts.boot_timeout.is_zero() {
            return Err(FlyConfigError::BootTimeout);
        }
        if parts.warm_pool_size > parts.max_leases {
            return Err(FlyConfigError::WarmPoolExceedsCapacity);
        }
        if parts.volume.is_some() && (parts.max_leases != 1 || parts.warm_pool_size != 0) {
            return Err(FlyConfigError::VolumeNeedsSingleLease);
        }
        if parts.workspace_volume.is_some() {
            if parts.volume.is_some() {
                return Err(FlyConfigError::WorkspaceVolumeWithOperatorVolume);
            }
            if parts.warm_pool_size != 0 {
                return Err(FlyConfigError::WorkspaceVolumeWithWarmPool);
            }
        }
        Ok(Self {
            image: parts.image,
            region: parts.region,
            state_dir: parts.state_dir,
            boot_timeout: parts.boot_timeout,
            max_leases: parts.max_leases,
            memory_mib: parts.memory_mib,
            cpus: parts.cpus,
            warm_pool_size: parts.warm_pool_size,
            volume: parts.volume,
            workspace_volume: parts.workspace_volume,
            egress_proxy: parts.egress_proxy,
            public_host: parts.public_host,
            preview_network: parts.preview_network,
        })
    }

    pub fn image(&self) -> &str {
        &self.image
    }

    pub fn region(&self) -> &FlyRegion {
        &self.region
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn max_leases(&self) -> usize {
        self.max_leases
    }

    pub fn warm_pool_size(&self) -> usize {
        self.warm_pool_size
    }

    pub fn volume(&self) -> Option<&FlyVolumeBinding> {
        self.volume.as_ref()
    }

    pub fn workspace_volume(&self) -> Option<&FlyWorkspaceVolume> {
        self.workspace_volume.as_ref()
    }
}
