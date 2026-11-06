use trogonai_proto::platform::workload_identity::v1alpha1::CredentialActivation;

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
        Ok(Self(message))
    }
}

impl CredentialActivationFacts {
    pub fn message(&self) -> &CredentialActivation {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("activation binding does not match the audit scope")]
pub struct ActivationScopeError;
