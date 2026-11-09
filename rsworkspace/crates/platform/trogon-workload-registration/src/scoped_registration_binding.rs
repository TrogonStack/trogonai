use trogon_workload_identity::{RegistrationBinding, WorkloadRole};
use trogonai_proto::platform::workload_identity::v1alpha1::{
    RegistrationBinding as WireBinding, WorkloadRole as WireRole,
};

use crate::{CanonicalAgentId, ProducerConfig, RegistrationError};

#[derive(Debug)]
pub(crate) struct ScopedRegistrationBinding(RegistrationBinding);

impl ScopedRegistrationBinding {
    pub(crate) fn decode(config: &ProducerConfig, binding: &WireBinding) -> Result<Self, RegistrationError> {
        let role = match binding.role.as_known() {
            Some(WireRole::EgressProxy) => WorkloadRole::EgressProxy,
            Some(WireRole::Agent) => WorkloadRole::Agent,
            Some(WireRole::Supervisor) => WorkloadRole::Supervisor,
            _ => return Err(RegistrationError::Scope),
        };
        if role == WorkloadRole::Agent {
            CanonicalAgentId::new(
                binding
                    .application_id
                    .as_deref()
                    .ok_or(RegistrationError::ApplicationIdentity)?,
            )?;
        }
        let binding = RegistrationBinding::new(
            &binding.spiffe_id,
            &binding.tenant,
            &binding.project,
            role,
            binding.application_id.as_deref(),
        )
        .map_err(RegistrationError::Binding)?;
        if binding.tenant() != config.scope().tenant().as_str()
            || binding.project() != config.scope().project().as_str()
            || binding.spiffe_id().trust_domain() != config.scope().trust_domain()
        {
            return Err(RegistrationError::Scope);
        }
        Ok(Self(binding))
    }

    pub(crate) fn into_binding(self) -> RegistrationBinding {
        self.0
    }
}
