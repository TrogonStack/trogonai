use super::{ReleaseTimeout, RuntimeService, ServiceXkey};

/// What a [`ReleaseClient`](crate::ReleaseClient) is configured with once, at
/// startup: who it asks as, which service key it trusts, and how long it
/// waits.
#[derive(Clone, Debug)]
pub struct ReleaseClientConfig {
    runtime_service: RuntimeService,
    service_xkey: ServiceXkey,
    timeout: ReleaseTimeout,
}

impl ReleaseClientConfig {
    pub fn new(runtime_service: RuntimeService, service_xkey: ServiceXkey) -> Self {
        Self {
            runtime_service,
            service_xkey,
            timeout: ReleaseTimeout::default(),
        }
    }

    #[must_use = "with_* setters return `self` by value; assign or chain the result"]
    pub fn with_timeout(mut self, timeout: ReleaseTimeout) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn runtime_service(&self) -> &RuntimeService {
        &self.runtime_service
    }

    pub fn service_xkey(&self) -> &ServiceXkey {
        &self.service_xkey
    }

    pub const fn timeout(&self) -> ReleaseTimeout {
        self.timeout
    }
}

#[cfg(test)]
mod tests;
