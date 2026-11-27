use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::Duration;

use crate::{FlyPreviewNetwork, FlyRegion, FlyVolumeBinding, FlyWorkspaceVolume};

/// The unchecked settings a [`crate::FlyConfig`] is parsed from.
#[derive(Debug, Clone)]
pub struct FlyConfigParts {
    pub image: String,
    pub region: FlyRegion,
    pub state_dir: PathBuf,
    pub boot_timeout: Duration,
    pub max_leases: usize,
    pub memory_mib: NonZeroU32,
    pub cpus: NonZeroU32,
    pub warm_pool_size: usize,
    pub volume: Option<FlyVolumeBinding>,
    /// Worker-owned per-lease workspace volume; `None` keeps the workspace on the Machine's ephemeral root
    /// filesystem.
    pub workspace_volume: Option<FlyWorkspaceVolume>,
    pub egress_proxy: Option<String>,
    /// Public hostname of the app's Fly Proxy; `None` means `{app}.fly.dev`.
    pub public_host: Option<String>,
    pub preview_network: FlyPreviewNetwork,
}
