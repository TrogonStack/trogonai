use serde::Serialize;

use crate::ResourcePoolId;

#[derive(Serialize)]
pub(crate) struct CloneBody<'a> {
    pub(crate) newid: u32,
    pub(crate) name: &'a str,
    pub(crate) full: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pool: Option<&'a ResourcePoolId>,
}

/// The guest firewall options that drop everything not explicitly allowed and pin the NIC's addresses.
#[derive(Serialize)]
pub(crate) struct FirewallOptionsBody {
    pub(crate) enable: u8,
    pub(crate) policy_in: &'static str,
    pub(crate) policy_out: &'static str,
    pub(crate) dhcp: u8,
    pub(crate) ndp: u8,
    pub(crate) radv: u8,
    pub(crate) macfilter: u8,
    pub(crate) ipfilter: u8,
}

impl FirewallOptionsBody {
    pub(crate) const fn locked_down() -> Self {
        Self {
            enable: 1,
            policy_in: "DROP",
            policy_out: "DROP",
            dhcp: 0,
            ndp: 0,
            radv: 0,
            macfilter: 1,
            ipfilter: 1,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct FirewallRuleBody {
    #[serde(rename = "type")]
    pub(crate) direction: &'static str,
    pub(crate) action: &'static str,
    pub(crate) proto: &'static str,
    pub(crate) dest: String,
    pub(crate) dport: String,
    pub(crate) enable: u8,
    pub(crate) pos: u8,
}

#[derive(Serialize)]
pub(crate) struct IpSetBody<'a> {
    pub(crate) name: &'a str,
}

#[derive(Serialize)]
pub(crate) struct IpSetEntryBody {
    pub(crate) cidr: String,
}

/// The guest's single NIC, attached to the proxy bridge with the firewall in the path.
#[derive(Serialize)]
pub(crate) struct NicBody {
    pub(crate) net0: String,
    pub(crate) ipconfig0: String,
}

/// What a fresh clone is configured with before it boots.
#[derive(Serialize)]
pub(crate) struct VmConfigBody<'a> {
    pub(crate) name: &'a str,
    pub(crate) agent: &'static str,
    pub(crate) ciuser: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) delete: Option<String>,
    pub(crate) memory: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cores: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cpulimit: Option<f64>,
}

/// A snapshot that keeps the VM's memory, so a rollback resumes the running guest.
#[derive(Serialize)]
pub(crate) struct SnapshotBody<'a> {
    pub(crate) snapname: &'a str,
    pub(crate) vmstate: u8,
}

#[derive(Serialize)]
pub(crate) struct ExecBody<'a> {
    pub(crate) command: &'a [String],
}
