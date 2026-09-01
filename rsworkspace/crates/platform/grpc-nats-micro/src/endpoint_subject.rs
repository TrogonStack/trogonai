//! The subject one `rpc` method is reachable on, derived per ADR 0016 §2 as
//! `<subject_prefix>.<service-name>.<MethodName>`.

use std::sync::Arc;

use trogon_nats::subject_conformance::{SubjectViolationError, validate_published_subject};

use crate::method_name::MethodName;
use crate::service_name::ServiceName;
use crate::subject_prefix::SubjectPrefix;

/// Why the subject derived from otherwise valid components is not a subject
/// this binding may publish to.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("derived endpoint subject {subject:?} is not a conformant published subject")]
pub struct EndpointSubjectError {
    pub subject: String,
    #[source]
    pub source: SubjectViolationError,
}

/// A NATS subject derived from a subject prefix, service name, and method
/// name. Always constructed through [`EndpointSubject::new`], so the ADR 0016
/// §2 derivation rule cannot drift out of sync at a call site.
///
/// Each component validates itself, which leaves only the whole-subject
/// budget (token count, byte length) to check here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointSubject(Arc<str>);

impl EndpointSubject {
    pub fn new(
        subject_prefix: &SubjectPrefix,
        service_name: &ServiceName,
        method_name: &MethodName,
    ) -> Result<Self, EndpointSubjectError> {
        let subject = format!("{subject_prefix}.{service_name}.{method_name}");
        validate_published_subject(&subject).map_err(|source| EndpointSubjectError {
            subject: subject.clone(),
            source,
        })?;
        Ok(Self(subject.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EndpointSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
