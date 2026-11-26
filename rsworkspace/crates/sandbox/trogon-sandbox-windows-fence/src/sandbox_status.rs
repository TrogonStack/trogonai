use crate::lease_proxy_ports_error::LeaseProxyPortsError;
use crate::proxy_port_range::ProxyPortRange;
use crate::sandbox_user_status::SandboxUserStatus;
use crate::wfp_state::WfpState;
use crate::wfp_status::WfpStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxStatus {
    pub user: SandboxUserStatus,
    pub wfp: WfpStatus,
}

impl SandboxStatus {
    /// The loopback ports a lease proxy may listen on, when the fence is in
    /// place for leases: the account provisioned and the filters installed
    /// with loopback closed, so the proxy is the only way out.
    pub fn lease_proxy_ports(&self) -> Result<ProxyPortRange, LeaseProxyPortsError> {
        if !self.user.provisioned {
            return Err(LeaseProxyPortsError::NotProvisioned);
        }
        if self.wfp.state != WfpState::Installed {
            return Err(LeaseProxyPortsError::FiltersNotInstalled {
                state: self.wfp.state.as_str().to_string(),
            });
        }
        if self.wfp.allow_loopback {
            return Err(LeaseProxyPortsError::LoopbackOpen);
        }
        self.wfp.port_range.ok_or(LeaseProxyPortsError::NoPortRange)
    }
}
