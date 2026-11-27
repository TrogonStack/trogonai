use serde::{Deserialize, Serialize};

use crate::{DomainPattern, HeaderCondition, HttpMethod, PathPrefix, RequestRuleAction};

/// One per-request rule of a [`NetworkPolicy`](crate::NetworkPolicy), applied after the policy has let the
/// destination through.
///
/// Every condition that is set must hold: the request goes to one of `domains` (any destination when empty), uses
/// one of `methods` (any when empty), sits under `path_prefix` (any path when absent), and carries every header in
/// `headers`. The first matching rule decides, and a request no rule matches is allowed. The contract carries the
/// rule only; it does not evaluate it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestRule {
    pub action: RequestRuleAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<DomainPattern>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<HttpMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<PathPrefix>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderCondition>,
}
