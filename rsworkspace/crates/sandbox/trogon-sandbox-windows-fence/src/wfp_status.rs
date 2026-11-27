use crate::proxy_port_range::ProxyPortRange;
use crate::wfp_state::WfpState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WfpStatus {
    pub state: WfpState,
    pub filters: u32,
    pub port_range: Option<ProxyPortRange>,
    pub allow_loopback: bool,
    pub user_sid: Option<String>,
    pub hint: Option<String>,
}
