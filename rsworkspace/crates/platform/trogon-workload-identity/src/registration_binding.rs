use spiffe::SpiffeId;

use crate::workload_id::WorkloadId;
use crate::workload_project::WorkloadProject;
use crate::workload_tenant::WorkloadTenant;
use crate::{ApplicationId, RegistrationError, WorkloadRole};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationBinding {
    spiffe_id: WorkloadId,
    tenant: WorkloadTenant,
    project: WorkloadProject,
    role: WorkloadRole,
    application_id: Option<ApplicationId>,
}

impl RegistrationBinding {
    pub fn new(
        spiffe_id: &str,
        tenant: &str,
        project: &str,
        role: WorkloadRole,
        application_id: Option<&str>,
    ) -> Result<Self, RegistrationError> {
        let spiffe_id = WorkloadId::new(spiffe_id)?;
        let tenant = WorkloadTenant::new(tenant)?;
        let project = WorkloadProject::new(project)?;
        let application_id = application_id.map(ApplicationId::new).transpose()?;
        if (role == WorkloadRole::Agent) != application_id.is_some() {
            return Err(RegistrationError::Binding);
        }
        Ok(Self {
            spiffe_id,
            tenant,
            project,
            role,
            application_id,
        })
    }

    pub fn spiffe_id(&self) -> &SpiffeId {
        self.spiffe_id.as_spiffe_id()
    }
    pub fn tenant(&self) -> &str {
        self.tenant.as_str()
    }
    pub fn project(&self) -> &str {
        self.project.as_str()
    }
    pub fn role(&self) -> WorkloadRole {
        self.role
    }
    pub fn application_id(&self) -> Option<&ApplicationId> {
        self.application_id.as_ref()
    }
}
