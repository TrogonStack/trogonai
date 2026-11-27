use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_protocol::{tenant_scoped_kv_key, tenant_scoped_kv_prefix};

/// Every tenant-scoped usage key is `tenant.<tenant>.<lease>`, three subject
/// tokens, so one tenant's keys match a `tenant.<tenant>.>` filter and a
/// legacy key (a bare lease id, one token) matches `*` alone. The tenant scan
/// asks the server for those two filters instead of listing the bucket.
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(constant_outside_constants_module, reason = "the constant is private to this module")
)]
pub(super) const SCOPED_KEY_PREFIX: &str = "tenant.";

/// The layout before usage keys carried their tenant: the bare lease id.
pub(super) fn legacy_kv_key(lease_id: &LeaseId) -> String {
    lease_id.as_str().replace(':', "_")
}

pub(super) fn nats_kv_key(tenant: &TenantId, lease_id: &LeaseId) -> String {
    let name = tenant_scoped_kv_key(tenant, &legacy_kv_key(lease_id));
    let lease = &name[tenant_scoped_kv_prefix(tenant).len()..];
    format!("{}{lease}", nats_kv_tenant_prefix(tenant))
}

/// `tenant.<tenant>.`, shared by every key of the tenant. The escaped tenant
/// never contains a `.`, so the prefix cannot match another tenant.
pub(super) fn nats_kv_tenant_prefix(tenant: &TenantId) -> String {
    let escaped = tenant_scoped_kv_prefix(tenant);
    format!("{SCOPED_KEY_PREFIX}{}.", &escaped[..escaped.len() - 1])
}
