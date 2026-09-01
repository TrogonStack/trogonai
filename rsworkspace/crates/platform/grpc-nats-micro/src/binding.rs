//! Binding descriptors: subject derivation per ADR 0016 §2
//! (`<subject_prefix>.<service-name>.<MethodName>`).

/// A NATS subject derived from a subject prefix, service name, and method
/// name, per ADR 0016 §2. Always constructed through [`EndpointSubject::new`]
/// so the derivation rule cannot drift out of sync at a call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointSubject(String);

impl EndpointSubject {
    pub fn new(subject_prefix: &str, service_name: &str, method_name: &str) -> Self {
        Self(format!("{subject_prefix}.{service_name}.{method_name}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EndpointSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One `rpc` method of the annotated protobuf service, registered as a micro
/// endpoint on its derived subject.
#[derive(Debug, Clone)]
pub struct EndpointBinding {
    method_name: String,
    subject: EndpointSubject,
}

impl EndpointBinding {
    pub fn new(subject_prefix: &str, service_name: &str, method_name: impl Into<String>) -> Self {
        let method_name = method_name.into();
        let subject = EndpointSubject::new(subject_prefix, service_name, &method_name);
        Self { method_name, subject }
    }

    pub fn method_name(&self) -> &str {
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
    name: String,
    version: String,
    description: Option<String>,
    subject_prefix: String,
    endpoints: Vec<EndpointBinding>,
}

impl ServiceBinding {
    pub fn new(name: impl Into<String>, version: impl Into<String>, subject_prefix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            description: None,
            subject_prefix: subject_prefix.into(),
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
    #[must_use = "with_* setters return `self` by value; assign or chain the result"]
    pub fn with_method(mut self, method_name: impl Into<String>) -> Self {
        self.endpoints
            .push(EndpointBinding::new(&self.subject_prefix, &self.name, method_name));
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn endpoints(&self) -> &[EndpointBinding] {
        &self.endpoints
    }
}
