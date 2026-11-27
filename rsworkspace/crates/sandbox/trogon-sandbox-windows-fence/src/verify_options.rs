use crate::proxy_port_range::ProxyPortRange;
use crate::sandbox_user::SandboxUser;
use crate::sublayer_guid::SublayerGuid;

#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    /// Overrides the probe target (skips the local listener bind).
    pub target: Option<String>,
    /// The permit range the listener must stay out of.
    pub proxy_port_range: Option<ProxyPortRange>,
    pub allow_loopback: bool,
    /// Also make the off-loopback attempt under the anonymous token. It can
    /// take seconds; for diagnostics only.
    pub anonymous_egress: bool,
    pub sublayer_guid: Option<SublayerGuid>,
    pub sandbox_user: Option<SandboxUser>,
    pub write_roots: Vec<String>,
}
