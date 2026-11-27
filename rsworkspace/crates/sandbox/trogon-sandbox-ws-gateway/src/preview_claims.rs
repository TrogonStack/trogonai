use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{LeaseId, TenantId};

/// The facts a preview token vouches for.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreviewClaims {
    #[serde(rename = "t")]
    pub tenant: TenantId,
    #[serde(rename = "l")]
    pub lease_id: LeaseId,
    #[serde(rename = "p")]
    pub port: u16,
    /// Unix seconds.
    #[serde(rename = "e")]
    pub expires_at: u64,
}
