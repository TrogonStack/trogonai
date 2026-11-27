use crate::install_context::InstallContext;
use crate::proxy_port_range::ProxyPortRange;
use crate::sandbox_user::SandboxUser;
use crate::sublayer_guid::SublayerGuid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallOptions {
    pub sublayer_guid: Option<SublayerGuid>,
    pub proxy_port_range: Option<ProxyPortRange>,
    pub allow_loopback: bool,
    pub sandbox_user: Option<SandboxUser>,
    /// Replace an existing install whose configuration differs.
    pub force: bool,
    pub skip_profile_grant: bool,
    /// Paths inside the profile to keep unreadable in addition to the
    /// built-in exclusions.
    pub profile_exclude: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub profile_grant_timeout_ms: Option<u64>,
}

impl InstallOptions {
    pub fn context(&self) -> InstallContext {
        InstallContext {
            sublayer_guid: self.sublayer_guid.clone(),
            proxy_port_range: self.proxy_port_range,
            sandbox_user: self.sandbox_user.clone(),
            allow_loopback: self.allow_loopback,
        }
    }
}
