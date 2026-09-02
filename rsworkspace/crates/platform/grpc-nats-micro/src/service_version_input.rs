//! The service version exactly as the `service` annotation spelled it
//! (ADR 0016 §1).

/// Untrusted service version text, as it arrives in
/// `trogon.nats.micro.v1alpha1.ServiceOptions.version`. Carries no guarantee
/// that the value is a semantic version, which is the only shape NATS Services
/// admits; [`crate::ServiceVersion::from_input`] is the single conversion into
/// the domain value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceVersionInput(Box<str>);

impl ServiceVersionInput {
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
