use crate::proxy_port_range::ProxyPortRange;
use crate::sandbox_user::SandboxUser;
use crate::sublayer_guid::SublayerGuid;

/// The options of the install a session expects. A suggested install command
/// names only the ones that are not the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallContext {
    pub sublayer_guid: Option<SublayerGuid>,
    pub proxy_port_range: Option<ProxyPortRange>,
    pub sandbox_user: Option<SandboxUser>,
    pub allow_loopback: bool,
}
