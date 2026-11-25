use async_trait::async_trait;
use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_audit::{ActivationRecorder, AuditSink, CredentialActivationFacts};
use trogon_workload_identity::{ActivationAudit, ActivationAuditError, ActivationFact, WorkloadRole};
use trogonai_proto::platform::workload_identity::v1alpha1::{
    CredentialActivation, RegistrationBinding, WorkloadRole as WireRole,
};

pub struct WorkloadActivationAudit<S> {
    recorder: ActivationRecorder<S>,
}

impl<S: AuditSink> WorkloadActivationAudit<S> {
    pub fn new(recorder: ActivationRecorder<S>) -> Self {
        Self { recorder }
    }
}

#[async_trait]
impl<S: AuditSink> ActivationAudit for WorkloadActivationAudit<S> {
    async fn record(&self, fact: &ActivationFact) -> Result<(), ActivationAuditError> {
        let binding = fact.binding();
        let role = match binding.role() {
            WorkloadRole::EgressProxy => WireRole::EgressProxy,
            WorkloadRole::Agent => WireRole::Agent,
            WorkloadRole::Supervisor => WireRole::Supervisor,
        };
        let facts = CredentialActivationFacts::try_from(CredentialActivation {
            binding: MessageField::some(RegistrationBinding {
                spiffe_id: binding.spiffe_id().to_string(),
                tenant: binding.tenant().into(),
                project: binding.project().into(),
                role: EnumValue::from(role),
                application_id: binding.application_id().map(|id| id.as_str().to_owned()),
            }),
            recovery_fence: fact.recovery_fence().get(),
            registration_revision: fact.registration_revision().get(),
            trust_revision: fact.trust_revision().get(),
            source_revision: fact.source_revision(),
            activation_revision: fact.activation_revision().get(),
            leaf_sha256: fact.leaf_sha256().to_vec(),
            public_key_sha256: fact.public_key_sha256().to_vec(),
            bundle_sha256: fact.bundle_sha256().to_vec(),
            not_before: MessageField::some(Timestamp::from(fact.not_before())),
            expires_at: MessageField::some(Timestamp::from(fact.expires_at())),
        })
        .map_err(|_| ActivationAuditError::Unavailable)?;
        self.recorder
            .record(facts)
            .await
            .map(|_| ())
            .map_err(|_| ActivationAuditError::Unavailable)
    }
}
