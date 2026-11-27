use trogon_sandbox_contract::{DriveId, DriveRecord};
use trogon_sandbox_protocol::{legacy_tenant_scoped_kv_key, tenant_scoped_kv_key};

pub fn drive_kv_key(drive_id: &DriveId) -> String {
    tenant_scoped_kv_key(&drive_id.tenant, drive_id.name.as_str())
}

/// Key a drive was stored under before keys became injective. Records there
/// are only trusted after their own id matches, since the key is shared by
/// every pair that folds to it.
pub(super) fn legacy_drive_kv_key(drive_id: &DriveId) -> String {
    legacy_tenant_scoped_kv_key(&drive_id.tenant, drive_id.name.as_str())
}

pub(super) fn owns(record: &DriveRecord, drive_id: &DriveId) -> bool {
    record.id == *drive_id && record.tenant == drive_id.tenant
}
