use std::path::PathBuf;
use std::time::Duration;

use crate::{FuseOtherUsers, FusePlatform, VolumeEndpoint, VolumeRegion};

#[derive(Debug, Clone)]
pub struct FuseMountConfig {
    pub mount_s3_binary: PathBuf,
    pub region: Option<VolumeRegion>,
    pub endpoint: Option<VolumeEndpoint>,
    pub other_users: FuseOtherUsers,
    pub platform: FusePlatform,
    /// How long a mount may take to appear in the mount table.
    pub ready_timeout: Duration,
    pub ready_poll_interval: Duration,
    /// The table listing active mounts, read to detect readiness.
    pub mount_table: PathBuf,
    pub unmount_binary: PathBuf,
    pub fallback_unmount_binary: PathBuf,
}

impl Default for FuseMountConfig {
    fn default() -> Self {
        Self {
            mount_s3_binary: PathBuf::from("mount-s3"),
            region: None,
            endpoint: None,
            other_users: FuseOtherUsers::Denied,
            platform: FusePlatform::current(),
            ready_timeout: Duration::from_secs(10),
            ready_poll_interval: Duration::from_millis(100),
            mount_table: PathBuf::from("/proc/mounts"),
            unmount_binary: PathBuf::from("fusermount3"),
            fallback_unmount_binary: PathBuf::from("umount"),
        }
    }
}
