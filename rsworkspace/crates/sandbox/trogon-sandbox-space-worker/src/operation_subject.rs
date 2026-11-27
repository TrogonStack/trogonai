use trogon_sandbox_contract::LeaseId;

use crate::{OperationSubjectError, OperationVerb};

/// A parsed `sandbox.provider.space.<verb>.<lease>` subject.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OperationSubject {
    verb: OperationVerb,
    lease: LeaseId,
}

impl OperationSubject {
    /// Parses the subject of a provider operation.
    pub fn parse(subject: &str) -> Result<Self, OperationSubjectError> {
        let tokens: Vec<&str> = subject.split('.').collect();
        match tokens.as_slice() {
            ["sandbox", "provider", "space", verb, lease] if !verb.is_empty() && !lease.is_empty() => Ok(Self {
                verb: OperationVerb::parse(verb),
                lease: LeaseId::new(*lease),
            }),
            _ => Err(OperationSubjectError(subject.to_owned())),
        }
    }

    /// The operation the subject names.
    pub fn verb(&self) -> &OperationVerb {
        &self.verb
    }

    /// The lease the subject names.
    pub fn lease(&self) -> &LeaseId {
        &self.lease
    }
}
