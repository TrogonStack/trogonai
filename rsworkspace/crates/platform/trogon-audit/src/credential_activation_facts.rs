use buffa_types::google::protobuf::Timestamp;
use spiffe::SpiffeId;
use trogonai_proto::platform::workload_identity::v1alpha1::{CredentialActivation, RegistrationBinding, WorkloadRole};

use crate::constants::{
    MAX_ACTIVATION_VALIDITY_SECONDS, MAX_PROTOBUF_TIMESTAMP_SECONDS, MAX_WORKLOAD_APPLICATION_ID_BYTES,
    MAX_WORKLOAD_SCOPE_BYTES, MAX_WORKLOAD_SPIFFE_ID_BYTES, NANOSECONDS_PER_SECOND, WORKLOAD_PUBLIC_FINGERPRINT_BYTES,
};

/// Bounded public activation evidence. This validates the audit contract; the
/// supervisor separately verifies attestation, certificate path and live authority.
#[derive(Debug, Clone, PartialEq)]
pub struct CredentialActivationFacts(CredentialActivation);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ActivationFactsError {
    #[error("activation binding is invalid")]
    Binding,
    #[error("activation fingerprint is invalid")]
    Fingerprint,
    #[error("activation generation is invalid")]
    Generation,
    #[error("activation validity is invalid")]
    Validity,
}

impl TryFrom<CredentialActivation> for CredentialActivationFacts {
    type Error = ActivationFactsError;

    fn try_from(message: CredentialActivation) -> Result<Self, Self::Error> {
        let binding = message.binding.as_option().ok_or(ActivationFactsError::Binding)?;
        if !valid_binding(binding) {
            return Err(ActivationFactsError::Binding);
        }
        if [
            message.recovery_fence,
            message.registration_revision,
            message.trust_revision,
            message.source_revision,
            message.activation_revision,
        ]
        .contains(&0)
        {
            return Err(ActivationFactsError::Generation);
        }
        if [&message.leaf_sha256, &message.public_key_sha256, &message.bundle_sha256]
            .iter()
            .any(|digest| digest.len() != WORKLOAD_PUBLIC_FINGERPRINT_BYTES)
        {
            return Err(ActivationFactsError::Fingerprint);
        }
        let before = message
            .not_before
            .as_option()
            .and_then(timestamp_nanos)
            .ok_or(ActivationFactsError::Validity)?;
        let expires = message
            .expires_at
            .as_option()
            .and_then(timestamp_nanos)
            .ok_or(ActivationFactsError::Validity)?;
        if expires <= before || expires - before > i128::from(MAX_ACTIVATION_VALIDITY_SECONDS) * NANOSECONDS_PER_SECOND
        {
            return Err(ActivationFactsError::Validity);
        }
        Ok(Self(message))
    }
}

impl CredentialActivationFacts {
    pub fn message(&self) -> &CredentialActivation {
        &self.0
    }

    pub(crate) fn matches_scope(&self, tenant: &str, project: &str) -> bool {
        self.0.binding.tenant == tenant && self.0.binding.project == project
    }

    pub(crate) fn into_message(self) -> CredentialActivation {
        self.0
    }
}

pub(crate) fn valid_binding(binding: &RegistrationBinding) -> bool {
    if !bounded_name(&binding.tenant, MAX_WORKLOAD_SCOPE_BYTES, false)
        || !bounded_name(&binding.project, MAX_WORKLOAD_SCOPE_BYTES, false)
        || binding.spiffe_id.len() > MAX_WORKLOAD_SPIFFE_ID_BYTES
        || SpiffeId::try_from(binding.spiffe_id.as_str()).is_err()
    {
        return false;
    }
    match (binding.role.as_known(), binding.application_id.as_deref()) {
        (Some(WorkloadRole::Agent), Some(application)) => {
            bounded_name(application, MAX_WORKLOAD_APPLICATION_ID_BYTES, true)
        }
        (Some(WorkloadRole::EgressProxy | WorkloadRole::Supervisor), None) => true,
        _ => false,
    }
}

fn bounded_name(value: &str, limit: usize, allow_colon: bool) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.') || (allow_colon && byte == b':')
        })
}

pub(crate) fn timestamp_nanos(timestamp: &Timestamp) -> Option<i128> {
    (timestamp.seconds > 0
        && timestamp.seconds <= MAX_PROTOBUF_TIMESTAMP_SECONDS
        && (0..NANOSECONDS_PER_SECOND).contains(&i128::from(timestamp.nanos)))
    .then(|| i128::from(timestamp.seconds) * NANOSECONDS_PER_SECOND + i128::from(timestamp.nanos))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("activation binding does not match the audit scope")]
pub struct ActivationScopeError;
