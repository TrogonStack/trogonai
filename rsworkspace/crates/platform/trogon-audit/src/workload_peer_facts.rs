use buffa_types::google::protobuf::Timestamp;
use trogonai_proto::platform::workload_identity::v1alpha1::{WorkloadPeerEvidence, WorkloadRole};

use crate::constants::WORKLOAD_PUBLIC_FINGERPRINT_BYTES;
use crate::credential_activation_facts::{timestamp_nanos, valid_binding};

/// Previously authenticated peer evidence and the listener's verification context.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadPeerEvidenceFacts(WorkloadPeerEvidence);

impl Eq for WorkloadPeerEvidenceFacts {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkloadPeerFactsError {
    #[error("workload peer binding is invalid")]
    Binding,
    #[error("workload peer fingerprint is invalid")]
    Fingerprint,
    #[error("workload peer generation is invalid")]
    Generation,
    #[error("workload peer path validity is invalid")]
    Validity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("workload peer evidence does not match the enforced egress decision")]
pub struct WorkloadPeerScopeError;

impl TryFrom<WorkloadPeerEvidence> for WorkloadPeerEvidenceFacts {
    type Error = WorkloadPeerFactsError;

    fn try_from(message: WorkloadPeerEvidence) -> Result<Self, Self::Error> {
        let binding = message
            .peer_binding
            .as_option()
            .ok_or(WorkloadPeerFactsError::Binding)?;
        if !valid_binding(binding) || binding.role.as_known() != Some(WorkloadRole::Agent) {
            return Err(WorkloadPeerFactsError::Binding);
        }
        if [&message.peer_leaf_sha256, &message.verifier_bundle_sha256]
            .iter()
            .any(|fingerprint| fingerprint.len() != WORKLOAD_PUBLIC_FINGERPRINT_BYTES)
        {
            return Err(WorkloadPeerFactsError::Fingerprint);
        }
        if [
            message.recovery_fence,
            message.registration_revision,
            message.trust_revision,
            message.verifier_source_revision,
            message.verifier_activation_revision,
        ]
        .contains(&0)
        {
            return Err(WorkloadPeerFactsError::Generation);
        }
        if message
            .peer_path_expires_at
            .as_option()
            .and_then(timestamp_nanos)
            .is_none()
        {
            return Err(WorkloadPeerFactsError::Validity);
        }
        Ok(Self(message))
    }
}

impl WorkloadPeerEvidenceFacts {
    pub fn message(&self) -> &WorkloadPeerEvidence {
        &self.0
    }

    pub(crate) fn matches_binding(&self, tenant: &str, project: &str, application: &str) -> bool {
        self.0.peer_binding.tenant == tenant
            && self.0.peer_binding.project == project
            && self.0.peer_binding.application_id.as_deref() == Some(application)
    }

    pub(crate) fn valid_at(&self, occurred_at: &Timestamp) -> bool {
        timestamp_nanos(&self.0.peer_path_expires_at)
            .zip(timestamp_nanos(occurred_at))
            .is_some_and(|(expires, occurred)| occurred < expires)
    }

    pub(crate) fn into_message(self) -> WorkloadPeerEvidence {
        self.0
    }
}

#[cfg(test)]
mod tests;
