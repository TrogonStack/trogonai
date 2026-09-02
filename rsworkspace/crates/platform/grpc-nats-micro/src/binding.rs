//! Binding descriptors: the annotated protobuf service and its `rpc` methods,
//! bound to NATS micro per ADR 0016 §1 and §2.

use crate::discovery_metadata::DiscoveryMetadata;
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
    metadata: DiscoveryMetadata,
}

impl EndpointBinding {
    pub fn new(
        subject_prefix: &SubjectPrefix,
        service_name: &ServiceName,
        method_name: MethodName,
        metadata: DiscoveryMetadata,
    ) -> Result<Self, EndpointSubjectError> {
        let subject = EndpointSubject::new(subject_prefix, service_name, &method_name)?;
        Ok(Self {
            method_name,
            subject,
            metadata,
        })
    }

    pub fn method_name(&self) -> &MethodName {
        &self.method_name
    }

    pub fn subject(&self) -> &EndpointSubject {
        &self.subject
    }

    /// `MethodOptions.metadata`, which populates this endpoint's discovery
    /// record (ADR 0016 §1).
    pub const fn metadata(&self) -> &DiscoveryMetadata {
        &self.metadata
    }
}

/// The annotated protobuf service registered as one NATS micro service
/// (ADR 0016 §1), and the subject prefix its endpoints are derived under.
#[derive(Debug, Clone)]
pub struct ServiceBinding {
    name: ServiceName,
    version: ServiceVersion,
    description: Option<String>,
    metadata: DiscoveryMetadata,
    subject_prefix: SubjectPrefix,
    endpoints: Vec<EndpointBinding>,
}

impl ServiceBinding {
    pub fn new(name: ServiceName, version: ServiceVersion, subject_prefix: SubjectPrefix) -> Self {
        Self {
            name,
            version,
            description: None,
            metadata: DiscoveryMetadata::default(),
            subject_prefix,
            endpoints: Vec::new(),
        }
    }

    #[must_use = "with_* setters return `self` by value; assign or chain the result"]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use = "with_* setters return `self` by value; assign or chain the result"]
    pub fn with_metadata(mut self, metadata: DiscoveryMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Register an `rpc` method as a micro endpoint, deriving its subject
    /// from this binding's subject prefix and service name.
    ///
    /// `metadata` is the method's own `MethodOptions.metadata`. Every endpoint
    /// has a discovery record, so a method that declares none passes an empty
    /// map rather than leaving the argument out.
    pub fn with_method(
        mut self,
        method_name: MethodName,
        metadata: DiscoveryMetadata,
    ) -> Result<Self, EndpointSubjectError> {
        self.endpoints.push(EndpointBinding::new(
            &self.subject_prefix,
            &self.name,
            method_name,
            metadata,
        )?);
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

    /// `ServiceOptions.metadata`, which populates this service's discovery
    /// record (ADR 0016 §1).
    pub const fn metadata(&self) -> &DiscoveryMetadata {
        &self.metadata
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
