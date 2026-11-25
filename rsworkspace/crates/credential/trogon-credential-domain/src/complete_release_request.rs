use super::{InjectionLocation, ReleaseHost, RuntimeServiceId};

/// What a caller asks to release, with every dimension the release check needs.
///
/// It exists because absence is never a wildcard: a request that omits its
/// runtime service, destination host, or injection location cannot be built,
/// so no check can quietly skip a dimension the caller forgot to name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteReleaseRequest {
    runtime_service: RuntimeServiceId,
    host: ReleaseHost,
    location: InjectionLocation,
}

impl CompleteReleaseRequest {
    pub fn new(runtime_service: RuntimeServiceId, host: ReleaseHost, location: InjectionLocation) -> Self {
        Self {
            runtime_service,
            host,
            location,
        }
    }

    pub fn runtime_service(&self) -> &RuntimeServiceId {
        &self.runtime_service
    }

    pub fn host(&self) -> &ReleaseHost {
        &self.host
    }

    pub fn location(&self) -> &InjectionLocation {
        &self.location
    }
}

#[cfg(test)]
mod tests;
