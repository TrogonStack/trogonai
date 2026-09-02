//! The registered NATS micro service's version (ADR 0016 §1).

use crate::service_version_input::ServiceVersionInput;

/// Why a [`ServiceVersion`] could not be constructed.
#[derive(Debug, thiserror::Error)]
#[error("service version is not a semantic version")]
pub struct ServiceVersionError(#[from] semver::Error);

/// A service version NATS micro will accept.
///
/// NATS Services (ADR-32) admits only semantic versions, and `async_nats`
/// rejects anything else when the service starts. Parsing at construction
/// moves that rejection off the startup path, so a binding that exists is one
/// that can register.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceVersion(Box<str>);

impl ServiceVersion {
    pub fn from_input(input: &ServiceVersionInput) -> Result<Self, ServiceVersionError> {
        let value = input.as_str();
        semver::Version::parse(value)?;
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ServiceVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
