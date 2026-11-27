use trogon_sandbox_contract::TenantId;

use crate::naming::Atespace;

pub(super) fn atespace_of(value: &str) -> Option<Atespace> {
    let tenant = TenantId::new(value).ok()?;
    Atespace::for_tenant(&tenant).ok()
}
