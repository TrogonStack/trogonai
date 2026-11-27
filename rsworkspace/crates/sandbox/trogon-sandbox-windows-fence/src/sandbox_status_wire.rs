use serde::Deserialize;

use crate::wfp_state::WfpState;

#[derive(Deserialize)]
pub(crate) struct WfpStatusWire {
    pub(crate) state: WfpState,
    pub(crate) filters: u32,
    pub(crate) port_range: Option<(u16, u16)>,
    pub(crate) allow_loopback: Option<bool>,
    pub(crate) user_sid: Option<String>,
    pub(crate) hint: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UserAccountWire {
    pub(crate) exists: bool,
    pub(crate) sid: Option<String>,
    pub(crate) group_exists: bool,
    pub(crate) group_sid: Option<String>,
    pub(crate) in_builtin_users: bool,
    pub(crate) in_sandbox_group: bool,
    pub(crate) hidden_from_logon: bool,
}

#[derive(Deserialize)]
pub(crate) struct GrantRecordWire {
    pub(crate) state: String,
    pub(crate) roots: Option<Vec<String>>,
    pub(crate) reason: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UserStatusWire {
    pub(crate) user: UserAccountWire,
    pub(crate) cred_present: bool,
    pub(crate) marker_version: Option<u32>,
    pub(crate) setup_version: Option<u32>,
    pub(crate) real_user_sid: String,
    pub(crate) ca_cert_thumb: Option<String>,
    pub(crate) ca_cert_pem: Option<String>,
    pub(crate) seclogon: Option<String>,
    pub(crate) persistent_grants: Option<GrantRecordWire>,
    pub(crate) profile_grant: Option<GrantRecordWire>,
    pub(crate) session_user_sid: Option<String>,
    pub(crate) capabilities: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(crate) struct SandboxStatusWire {
    pub(crate) user: UserStatusWire,
    pub(crate) wfp: WfpStatusWire,
}
