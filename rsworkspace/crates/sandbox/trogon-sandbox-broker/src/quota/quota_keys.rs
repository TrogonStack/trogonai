use chrono::{Datelike, Utc};
use trogon_sandbox_contract::TenantId;
use trogon_sandbox_protocol::tenant_scoped_kv_key;

pub fn quota_key(tenant: &TenantId) -> String {
    let now = Utc::now();
    format!("{}:{:04}-{:02}", tenant.as_str(), now.year(), now.month())
}

/// NATS KV key of `tenant`'s quota record for one month. Injective: the
/// tenant part never contains the `_` separator, so tenants `a:b` and `a_b`,
/// which shared the legacy key `a_b_{yyyy}-{mm}`, get distinct keys.
pub fn quota_kv_key_for_month(tenant: &TenantId, year: i32, month: u32) -> String {
    tenant_scoped_kv_key(tenant, &format!("{year:04}-{month:02}"))
}

/// Key the quota record was written under before the current layout:
/// `{tenant}:{yyyy}-{mm}` with `:` folded to `_`. Only read it, to seed the
/// current key once; never write it.
pub fn legacy_quota_kv_key_for_month(tenant: &TenantId, year: i32, month: u32) -> String {
    format!("{}:{year:04}-{month:02}", tenant.as_str()).replace(':', "_")
}

pub(super) fn nats_kv_keys(tenant: &TenantId) -> (String, String) {
    let now = Utc::now();
    (
        quota_kv_key_for_month(tenant, now.year(), now.month()),
        legacy_quota_kv_key_for_month(tenant, now.year(), now.month()),
    )
}
