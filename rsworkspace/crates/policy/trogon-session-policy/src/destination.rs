use trogon_sandbox_contract::{DomainPattern, PatternTarget};

use crate::{DestinationId, DestinationLocation, PolicyError};

/// One exact host and port. The native transport must pin resolved addresses.
#[derive(Debug, Clone)]
pub struct Destination {
    id: DestinationId,
    location: DestinationLocation,
    host: String,
    port: u16,
    pattern: DomainPattern,
}

impl Destination {
    pub fn new(id: DestinationId, location: DestinationLocation, host: String, port: u16) -> Result<Self, PolicyError> {
        if port == 0 || host.len() > 253 {
            return Err(PolicyError::Destination);
        }
        let pattern = DomainPattern::new(&format!("{host}:{port}"))?;
        if !matches!(pattern.target(), PatternTarget::Exact(_)) || pattern.to_string() != format!("{host}:{port}") {
            return Err(PolicyError::Destination);
        }
        Ok(Self {
            id,
            location,
            host,
            port,
            pattern,
        })
    }

    pub fn id(&self) -> &DestinationId {
        &self.id
    }
    pub fn location(&self) -> DestinationLocation {
        self.location
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub(crate) fn pattern(&self) -> DomainPattern {
        self.pattern.clone()
    }
}
