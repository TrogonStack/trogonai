//! Per-tenant OCI repository that the broker pushes checkpoints to and
//! restores tagged and prewarmed snapshots from.
//!
//! The legacy path, `snapshots/{tenant}` with `:` and `/` folded to `_`, sent
//! tenants `a:b`, `a/b` and `a_b` to one repository. The current path keeps
//! `snapshots/{tenant}` for every tenant without `:` or `/`, so those tenants
//! keep their repository, and moves the rest to
//! `snapshots/tenant-hex/{hex of the tenant id}`. A kept path has a single
//! segment after `snapshots/` and a moved path has two, so no two tenants
//! share a repository. The tenant named `tenant-hex` moves too, so no tenant
//! owns the prefix of the moved repositories in a registry that grants access
//! by repository prefix.

use trogon_sandbox_contract::TenantId;

const ROOT: &str = "snapshots";
const HEX_SEGMENT: &str = "tenant-hex";

/// Repository new snapshots of `tenant` are written to.
pub fn snapshot_repository_for_tenant(tenant: &TenantId) -> String {
    let id = tenant.as_str();
    if id.contains([':', '/']) || id == HEX_SEGMENT {
        let hex: String = id.bytes().map(|byte| format!("{byte:02x}")).collect();
        format!("{ROOT}/{HEX_SEGMENT}/{hex}")
    } else {
        format!("{ROOT}/{id}")
    }
}

/// Repository snapshots of `tenant` were written to before the current
/// layout. Equal to [`snapshot_repository_for_tenant`] unless the tenant id
/// contains `:` or `/` or is `tenant-hex`; only read or delete from it, never
/// write.
pub fn legacy_snapshot_repository_for_tenant(tenant: &TenantId) -> String {
    format!("{ROOT}/{}", tenant.as_str().replace([':', '/'], "_"))
}
