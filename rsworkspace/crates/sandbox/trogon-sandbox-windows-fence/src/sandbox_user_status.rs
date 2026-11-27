use crate::persistent_grants::PersistentGrants;
use crate::profile_grant_record::ProfileGrantRecord;
use crate::seclogon_state::SeclogonState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxUserStatus {
    pub provisioned: bool,
    pub sid: Option<String>,
    pub group_exists: bool,
    pub group_sid: Option<String>,
    pub in_builtin_users: bool,
    pub in_sandbox_group: bool,
    pub hidden_from_logon: bool,
    pub cred_present: bool,
    pub marker_version: Option<u32>,
    pub setup_version: Option<u32>,
    pub real_user_sid: String,
    pub ca_cert_thumb: Option<String>,
    pub ca_cert_pem: Option<String>,
    pub seclogon: Option<SeclogonState>,
    pub persistent_grants: Option<PersistentGrants>,
    pub profile_grant: Option<ProfileGrantRecord>,
    pub capabilities: Vec<String>,
    pub session_user_sid: Option<String>,
}

impl SandboxUserStatus {
    /// The PEM and thumbprint of the CA the
    /// sandbox user trusts, when both are recorded.
    pub fn ca_cert(&self) -> Option<(&str, &str)> {
        Some((self.ca_cert_pem.as_deref()?, self.ca_cert_thumb.as_deref()?))
    }
}
