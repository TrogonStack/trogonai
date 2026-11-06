use trogonai_proto::platform::workload_identity::v1alpha1::WorkloadPeerEvidence;

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
#[error("workload peer evidence does not match the enforced egress actor and scope")]
pub struct WorkloadPeerScopeError;

impl TryFrom<WorkloadPeerEvidence> for WorkloadPeerEvidenceFacts {
    type Error = WorkloadPeerFactsError;

    fn try_from(message: WorkloadPeerEvidence) -> Result<Self, Self::Error> {
        Ok(Self(message))
    }
}

impl WorkloadPeerEvidenceFacts {
    pub fn message(&self) -> &WorkloadPeerEvidence {
        &self.0
    }
}

#[cfg(test)]
mod tests;
