use std::path::PathBuf;
use std::time::Duration;

use crate::{ProxyNetwork, VmId};

/// The settings a [`ProxmoxConfig`](crate::ProxmoxConfig) is built from.
pub struct ProxmoxConfigParts {
    /// The template every lease is a linked clone of.
    pub template: VmId,
    /// The image reference leases must name; it describes the template.
    pub image: String,
    pub state_dir: PathBuf,
    pub boot_timeout: Duration,
    pub max_leases: usize,
    /// The isolated guest network. Without it guests have no NIC.
    pub network: Option<ProxyNetwork>,
}
