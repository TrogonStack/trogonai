use trogon_sandbox_contract::SnapshotTag;
use trogon_sandbox_protocol::{legacy_tenant_scoped_kv_key, tenant_scoped_kv_key};

pub fn tag_kv_key(tag: &SnapshotTag) -> String {
    tenant_scoped_kv_key(&tag.tenant, tag.name.as_str())
}

/// Key a tag was stored under before keys became injective.
pub(super) fn legacy_tag_kv_key(tag: &SnapshotTag) -> String {
    legacy_tenant_scoped_kv_key(&tag.tenant, tag.name.as_str())
}
