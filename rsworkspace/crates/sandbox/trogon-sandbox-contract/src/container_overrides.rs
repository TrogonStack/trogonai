use serde::{Deserialize, Serialize};

use crate::{ExtraCapability, PidsLimit, RootfsOverride, ShmSizeMib};

/// Loosening a lease may ask of the worker's container hardening, each unset by default.
///
/// There is no way to turn `no-new-privileges` off, to run privileged, or to lift the memory cap from here. Each
/// field checks its own range or grammar, so a value that parses is one a lease may ask for. Whether the tenant may
/// ask for overrides at all is a policy decision outside this type.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContainerOverrides {
    /// Replaces the worker's process ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_limit: Option<PidsLimit>,
    /// Leaves the root filesystem writable on a worker that makes it read only.
    #[serde(default)]
    pub writable_rootfs: RootfsOverride,
    /// Capabilities kept in addition to the worker's retained set, without the `CAP_` prefix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_capabilities: Vec<ExtraCapability>,
    /// Replaces the size of `/dev/shm`, which browsers need larger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shm_size_mib: Option<ShmSizeMib>,
}

impl ContainerOverrides {
    /// Reports whether nothing is overridden, so the wire can leave the field out.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
