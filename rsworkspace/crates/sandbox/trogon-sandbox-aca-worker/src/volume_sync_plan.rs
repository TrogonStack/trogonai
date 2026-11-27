use trogon_sandbox_volumes::{StoreResourceName, VolumeMode, VolumePrefix};

/// Where the changes of a read-write volume are pushed back to.
#[derive(Debug, Clone)]
pub struct VolumeSyncPlan {
    pub container: StoreResourceName,
    pub prefix: VolumePrefix,
    pub mode: VolumeMode,
    pub account: StoreResourceName,
}
