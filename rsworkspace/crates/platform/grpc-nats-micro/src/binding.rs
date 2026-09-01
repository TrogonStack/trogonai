//! Binding descriptors: the annotated protobuf service and its `rpc` methods,
//! bound to NATS micro per ADR 0016 §1 and §2.

use crate::endpoint_subject::{EndpointSubject, EndpointSubjectError};
use crate::method_name::MethodName;
use crate::service_name::ServiceName;
use crate::service_version::ServiceVersion;
use crate::subject_prefix::SubjectPrefix;

/// One `rpc` method of the annotated protobuf service, registered as a micro
/// endpoint on its derived subject.
#[derive(Debug, Clone)]
pub struct EndpointBinding {
    method_name: MethodName,
    subject: EndpointSubject,
}

impl EndpointBinding {
    pub fn new(
        subject_prefix: &SubjectPrefix,
        service_name: &ServiceName,
        method_name: MethodName,
    ) -> Result<Self, EndpointSubjectError> {
        let subject = EndpointSubject::new(subject_prefix, service_name, &method_name)?;
        Ok(Self { method_name, subject })
    }

    pub fn method_name(&self) -> &MethodName {
        &self.method_name
    }

    pub fn subject(&self) -> &EndpointSubject {
        &self.subject
    }
}

/// The annotated protobuf service registered as one NATS micro service
/// (ADR 0016 §1), and the subject prefix its endpoints are derived under.
#[derive(Debug, Clone)]
pub struct ServiceBinding {
    name: ServiceName,
    version: ServiceVersion,
    description: Option<String>,
    subject_prefix: SubjectPrefix,
    endpoints: Vec<EndpointBinding>,
}

impl ServiceBinding {
    pub fn new(name: ServiceName, version: ServiceVersion, subject_prefix: SubjectPrefix) -> Self {
        Self {
            name,
            version,
            description: None,
            subject_prefix,
            endpoints: Vec::new(),
        }
    }

    #[must_use = "with_* setters return `self` by value; assign or chain the result"]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Register an `rpc` method as a micro endpoint, deriving its subject
    /// from this binding's subject prefix and service name.
    pub fn with_method(mut self, method_name: MethodName) -> Result<Self, EndpointSubjectError> {
        self.endpoints
            .push(EndpointBinding::new(&self.subject_prefix, &self.name, method_name)?);
        Ok(self)
    }

    pub fn name(&self) -> &ServiceName {
        &self.name
    }

    pub const fn version(&self) -> &ServiceVersion {
        &self.version
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn subject_prefix(&self) -> &SubjectPrefix {
        &self.subject_prefix
    }

    pub fn endpoints(&self) -> &[EndpointBinding] {
        &self.endpoints
    }
}

#[cfg(test)]
mod tests;
