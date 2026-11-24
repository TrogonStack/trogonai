use serde::{Deserialize, Serialize};

use crate::{LeaseId, SandboxId, TenantId};

/// The claims an [`OidcToken`](crate::OidcToken) carries for a forwarded request.
///
/// `aud` is the exact forwarding URL, not just its origin, so a receiving proxy that fronts several paths on one
/// host can trust only the target the sandbox owner declared. `sub` names the sandbox, apart from `tenant_id` and
/// `lease_id`, so a receiver can authorize on any of the three. `iat` and `exp` are Unix seconds.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct OidcClaims {
    pub iss: String,
    pub sub: SandboxId,
    pub aud: String,
    pub tenant_id: TenantId,
    pub lease_id: LeaseId,
    pub iat: u64,
    pub exp: u64,
}
