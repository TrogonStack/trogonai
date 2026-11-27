use serde::{Deserialize, Serialize};

use crate::{DomainPattern, DomainRuleAction};

/// Egress allow-rule for one domain.
///
/// `domain` may use a single leading `*.` wildcard (for example `*.example.com` covers any strict subdomain, and
/// an exact domain covers only itself) and an optional `:port` suffix.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DomainRule {
    pub domain: DomainPattern,
    #[serde(default)]
    pub action: DomainRuleAction,
}
