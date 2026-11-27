use std::num::NonZeroU32;
use std::time::Duration;

use crate::FlyWorkspaceVolumeError;

/// Each lease gets a Fly volume the worker creates, mounts at the workspace path and deletes with the lease. It
/// is what makes native snapshots possible, because Fly snapshots volumes and not Machine filesystems.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct FlyWorkspaceVolume {
    size_gb: NonZeroU32,
    snapshot_poll_interval: Duration,
    snapshot_timeout: Duration,
}

impl FlyWorkspaceVolume {
    pub fn new(
        size_gb: NonZeroU32,
        snapshot_poll_interval: Duration,
        snapshot_timeout: Duration,
    ) -> Result<Self, FlyWorkspaceVolumeError> {
        if snapshot_poll_interval.is_zero() || snapshot_timeout.is_zero() {
            return Err(FlyWorkspaceVolumeError);
        }
        Ok(Self {
            size_gb,
            snapshot_poll_interval,
            snapshot_timeout,
        })
    }

    pub fn size_gb(&self) -> NonZeroU32 {
        self.size_gb
    }

    pub fn snapshot_poll_interval(&self) -> Duration {
        self.snapshot_poll_interval
    }

    pub fn snapshot_timeout(&self) -> Duration {
        self.snapshot_timeout
    }
}
