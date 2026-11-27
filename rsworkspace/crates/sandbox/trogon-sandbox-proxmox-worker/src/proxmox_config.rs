use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::constants::MAX_LEASES_LIMIT;
use crate::{ProxmoxConfigError, ProxmoxConfigParts, ProxyNetwork, VmId};

/// The settings of one Proxmox worker, checked for consistency.
#[derive(Clone)]
pub struct ProxmoxConfig {
    pub(crate) template: VmId,
    pub(crate) image: String,
    pub(crate) state_dir: PathBuf,
    pub(crate) boot_timeout: Duration,
    pub(crate) max_leases: usize,
    pub(crate) network: Option<ProxyNetwork>,
}

impl ProxmoxConfig {
    pub fn new(parts: ProxmoxConfigParts) -> Result<Self, ProxmoxConfigError> {
        if !(1..=MAX_LEASES_LIMIT).contains(&parts.max_leases) {
            return Err(ProxmoxConfigError::MaxLeases);
        }
        if parts.boot_timeout.is_zero() {
            return Err(ProxmoxConfigError::BootTimeout);
        }
        if let Some(network) = &parts.network {
            network.validate(parts.max_leases)?;
        }
        Ok(Self {
            template: parts.template,
            image: parts.image,
            state_dir: parts.state_dir,
            boot_timeout: parts.boot_timeout,
            max_leases: parts.max_leases,
            network: parts.network,
        })
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub fn max_leases(&self) -> usize {
        self.max_leases
    }
}
