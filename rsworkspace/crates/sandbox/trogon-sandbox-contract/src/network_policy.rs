use serde::{Deserialize, Serialize};

use crate::{AllowRange, DeniedDomain, DomainRule, IpRange, OpaqueTunnelPolicy, RequestRule};

/// Egress firewall mode for a sandbox.
///
/// `AllowAll` is unrestricted egress (the provider default). `DenyAll` denies all outbound traffic, including DNS.
/// `UserDefined` is deny-by-default with explicit allow and deny IP ranges plus per-domain rules; `deny_ranges` and
/// `denied_domains` take precedence over `allow_ranges` and `rules`, so they can carve out exceptions, for example
/// allow the internet but block one internal CIDR, or allow `*.example.com` but deny `admin.example.com`. An
/// `allow_ranges` entry may name a single address and a port, which allows that destination on that port only. The
/// contract carries the policy only; it does not evaluate it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    AllowAll,
    DenyAll,
    UserDefined {
        rules: Vec<DomainRule>,
        allow_ranges: Vec<AllowRange>,
        deny_ranges: Vec<IpRange>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        denied_domains: Vec<DeniedDomain>,
        /// Refused when omitted; always serialized, so an older reader that took omission to mean `Allow` never
        /// widens a policy written by a newer one.
        #[serde(default)]
        opaque_tunnels: OpaqueTunnelPolicy,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        request_rules: Vec<RequestRule>,
    },
}
