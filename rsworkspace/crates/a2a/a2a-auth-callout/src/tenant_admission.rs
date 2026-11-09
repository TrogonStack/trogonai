pub use crate::constants::ENV_TENANT_MEMBERSHIPS;
pub use crate::file_tenant_admission::FileTenantAdmission;
pub use crate::tenant_admission_decision::TenantAdmissionDecision;
pub use crate::tenant_admission_error::TenantAdmissionError;
pub use crate::tenant_admission_request::TenantAdmissionRequest;
pub use crate::tenant_membership::TenantMembership;
pub use crate::tenant_memberships::TenantMemberships;

#[async_trait::async_trait]
pub trait TenantAdmission: Send + Sync + 'static {
    async fn authorize(
        &self,
        request: TenantAdmissionRequest<'_>,
    ) -> Result<TenantAdmissionDecision, TenantAdmissionError>;
}
