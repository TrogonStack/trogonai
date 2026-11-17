use crate::admission_keys::TenantAccount;
use trogon_identity_types::SubjectPrincipal;

#[derive(Debug, Clone, Copy)]
pub struct TenantAdmissionRequest<'a> {
    tenant: &'a TenantAccount,
    principal: &'a SubjectPrincipal,
}

impl<'a> TenantAdmissionRequest<'a> {
    pub fn new(tenant: &'a TenantAccount, principal: &'a SubjectPrincipal) -> Self {
        Self { tenant, principal }
    }

    pub fn tenant(&self) -> &TenantAccount {
        self.tenant
    }

    pub fn principal(&self) -> &SubjectPrincipal {
        self.principal
    }
}
