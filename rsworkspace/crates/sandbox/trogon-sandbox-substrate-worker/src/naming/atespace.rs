use std::fmt;

use trogon_sandbox_contract::TenantId;

use super::NamingError;
use super::dns_label::validate_dns1123_label;

/// A Substrate atespace: the tenant isolation boundary.
///
/// Deliberately *not* normalized from the tenant id. Lowercasing or rewriting `_` to `-`
/// would let two distinct tenants land on the same atespace, which is a cross-tenant data
/// leak dressed up as a convenience. A tenant id that is not already a DNS-1123 label is
/// rejected, and because the worker resolves the atespace on the lease path, the rejection
/// surfaces before any actor exists.
#[derive(Debug, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct Atespace(String);

impl Atespace {
    pub fn for_tenant(tenant: &TenantId) -> Result<Self, NamingError> {
        validate_dns1123_label(tenant.as_str()).map_err(|reason| NamingError::TenantNotADnsLabel {
            tenant: tenant.as_str().to_string(),
            reason: reason.to_string(),
        })?;
        Ok(Self(tenant.as_str().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Atespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
