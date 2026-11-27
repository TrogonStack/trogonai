use trogon_sandbox_contract::TenantId;
use trogon_sandbox_protocol::{legacy_tenant_scoped_kv_key, tenant_scoped_kv_key};

pub(super) fn named_lease_key(tenant: &TenantId, name: &str) -> String {
    tenant_scoped_kv_key(tenant, name)
}

/// Key a name was indexed under before keys became injective. Only read as a
/// fallback and only cleared for the exact lease: the broker re-checks the
/// tenant of any lease it resolves, so a folded key shared with another
/// tenant never hands that tenant's lease over.
pub(super) fn legacy_named_lease_key(tenant: &TenantId, name: &str) -> String {
    legacy_tenant_scoped_kv_key(tenant, name)
}
